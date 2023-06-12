#![feature(let_else)]
#![feature(once_cell)]
#![feature(scoped_threads)]
use std::ffi::CString;
use std::fs::{self, DirEntry, File};
use std::io::BufWriter;
use std::thread;
use std::time::Instant;

use camino::Utf8PathBuf;
use color_eyre::eyre::{ensure, eyre, Context};
use color_eyre::owo_colors::OwoColorize;
use color_eyre::Result;
use ffmpeg_sys_next as f;
use gifski::progress::ProgressReporter;
use gifski::Repeat;
use humansize::{file_size_opts, FileSize};
use indicatif::{ProgressBar, ProgressStyle};

mod decoder;
use decoder::*;

fn main() -> Result<()> {
    color_eyre::install()?;
    let args = std::env::args().collect::<Vec<_>>();

    let (files, skipped) = if args.len() <= 1 {
        let mut files = fs::read_dir(".").wrap_err("failed to list files")?
            .filter_map(|r| match r {
                Ok(e) => check_webm(e).map(Ok),
                Err(e) => Some(Err(e)),
            })
            .collect::<Result<Vec<_>, _>>()?;

        let files_count = files.len();
        if files_count == 0 {
            println!("No input files are detected");
            return Ok(());
        }

        files.retain(|(_, gif)| {
            !match fs::metadata(gif) {
                Ok(m) => m.is_file() && m.len() != 0,
                Err(_) => false,
            }
        });
        files.sort_unstable_by(|(a, _), (b, _)| a.cmp(b));

        if files.is_empty() {
            println!("All input files are already transcoded");
            return Ok(());
        }

        let skipped = files_count - files.len();
        (files, skipped)
    } else {
        let mut files = Vec::with_capacity(1);
        for name in args.into_iter().skip(1) {
            let mut path = Utf8PathBuf::from(name);
            let mut metadata = fs::metadata(&path).wrap_err_with(|| eyre!("input file {}", path.clone()))?;
            while metadata.is_symlink() {
                path = Utf8PathBuf::from_path_buf(fs::read_link(&path).wrap_err_with(|| eyre!("input file {}", path.clone()))?)
                    .map_err(|p| eyre!("invalid utf-8 path: {:?}", p))?;
                metadata = fs::metadata(&path).wrap_err_with(|| eyre!("input file {}", path.clone()))?;
            }

            let gif = path.with_extension("gif");
            files.push((path, gif));
        }
        (files, 0)
    };

    print!("Transcoding {} {}", files.len(), if files.len() > 1 { "files" } else { "file" });
    if skipped > 0 {
        println!(" ({} skipped)", skipped);
    } else {
        println!();
    }

    let name_max_len = files.iter()
        .map(|(n, _)| n.file_name().unwrap_or_else(|| unreachable!()))
        .map(unicode_width::UnicodeWidthStr::width_cjk)
        .max().unwrap_or_else(|| unreachable!());
    let progress_style = ProgressStyle::default_bar()
        .template(" {prefix:.green.bright} {msg} [{bar:50}]{percent:>3}%")
        .progress_chars("=> ");

    for (input, output) in files {
        let name = input.file_name().unwrap_or_else(|| unreachable!()).to_owned();
        let time = Instant::now();

        let input = CString::new(input.into_string())?;
        let mut ctx = WebmContext::new(input.as_c_str()).wrap_err_with(|| format!("failed to parse webm file: {name}"))?;
        let duration = ctx.duration();
        let mut stream = ctx.best_stream()?;
        let fps = stream.fps();

        let estimated_frames = (duration * fps.0 as u64) / f::AV_TIME_BASE as u64 / fps.1 as u64;
        ensure!(estimated_frames > 0, "invalid duration");

        struct ProgressAdapter<'a>(&'a ProgressBar);

        impl ProgressReporter for ProgressAdapter<'_> {
            fn increase(&mut self) -> bool {
                self.0.inc(1);
                true
            }

            fn done(&mut self, _: &str) {}
        }

        let (mut collector, writer) = gifski::new(gifski::Settings {
            width: None,
            height: None,
            quality: 100,
            fast: false,
            repeat: Repeat::Infinite,
        })?;

        thread::scope(|scope| {
            let pb = ProgressBar::new(estimated_frames);
            pb.set_style(progress_style.clone());
            pb.set_message(left_pad(&name, name_max_len));
            pb.set_prefix("Processing");

            let handle = scope.spawn(move |_| {
                let mut decoder = stream.decode(VpxCodec::VP9)?;
                let mut frame_index = 0;
                while let Some((frame, pts)) = decoder.decode_frame()? {
                    // thread::sleep(std::time::Duration::from_millis(500));
                    collector.add_frame_rgba(frame_index, frame, pts)?;
                    frame_index += 1;
                }
                Result::<_>::Ok(())
            });

            let result = writer.write(BufWriter::new(File::create(&output)?),
                &mut ProgressAdapter(&pb)).map_err(Into::into);
            let result = match handle.join().unwrap().and(result) {
                Ok(_) => Result::<_>::Ok(()),
                Err(e) => {
                    fs::remove_file(&output).ok();
                    Err(e)
                },
            };

            pb.finish_and_clear();

            if result.is_ok() {
                let size = fs::metadata(&output)?.len();
                println!(
                    "Finished {} in {}s, {}",
                    output.file_name().unwrap_or_else(|| unreachable!()).bright_cyan(),
                    time.elapsed().as_secs(),
                    size.file_size(file_size_opts::CONVENTIONAL).unwrap_or_else(|_| unreachable!())
                );
            }
            result
        })?;
    }

    Ok(())
}

fn check_webm(entry: DirEntry) -> Option<(Utf8PathBuf, Utf8PathBuf)> {
    let mut file_type = entry.file_type().ok()?;
    if file_type.is_dir() {
        return None;
    }

    let mut path = entry.path();
    if path.extension().and_then(|ext| ext.to_str()) != Some("webm") {
        return None;
    }

    while file_type.is_symlink() {
        path = fs::read_link(path).ok()?;
        file_type = entry.file_type().ok()?;
    }

    if !file_type.is_file() {
        return None;
    }

    let webm = match Utf8PathBuf::from_path_buf(path) {
        Ok(p) => p,
        Err(p) => {
            eprintln!("Warning: skipping file with invalid utf-8 name: {:?}", p);
            return None;
        },
    };

    let gif = webm.with_extension("gif");
    Some((webm, gif))
}

fn left_pad(str: &str, target_width: usize) -> String {
    let input_width = unicode_width::UnicodeWidthStr::width_cjk(str);
    if target_width > input_width {
        let mut s = String::with_capacity(target_width);
        for _ in 0..(target_width - input_width) {
            s.push(' ');
        }
        s
    } else {
        str.to_string()
    }
}
