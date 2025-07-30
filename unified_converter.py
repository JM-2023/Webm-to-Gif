#!/usr/bin/env python3
"""
Unified TGS and WebM to GIF converter
Converts TGS (Telegram stickers) and WebM files to GIF with transparency support
"""

import os
import sys
import json
import time
import gzip
import subprocess
from pathlib import Path
from typing import List, Tuple, Optional, Iterator, Dict, Any
import argparse

# Import required packages (handled by virtual environment)
try:
    import numpy as np
    from PIL import Image
    import lottie
    from lottie.exporters.gif import export_gif
    from lottie.parsers.tgs import parse_tgs
    import humanize
except ImportError as e:
    print(f"Missing required package: {e}")
    print("Please run using the command file which sets up the virtual environment.")
    sys.exit(1)


class ProgressBar:
    """Simple progress bar for terminal output."""
    
    def __init__(self, total: int, prefix: str = "", width: int = 50):
        self.total = max(total, 1)
        self.current = 0
        self.prefix = prefix
        self.width = width
        self._last_percent = -1
    
    def update(self, n: int = 1):
        self.current = min(self.current + n, self.total)
        percent = int(100 * self.current / self.total)
        
        if percent != self._last_percent:
            self._last_percent = percent
            self._display()
    
    def _display(self):
        percent = int(100 * self.current / self.total)
        filled = int(self.width * self.current / self.total)
        bar = "=" * filled + ">" + " " * (self.width - filled - 1)
        sys.stdout.write(f"\r\033[92m\033[1mProcessing\033[0m {self.prefix} [{bar}] {percent:>3}%")
        sys.stdout.flush()
    
    def close(self):
        sys.stdout.write("\r\033[K")
        sys.stdout.flush()


class TgsConverter:
    """Convert TGS files to GIF with transparency support."""
    
    @staticmethod
    def convert_tgs_to_gif(tgs_path, output_path=None, fps=30, width=512, height=512):
        """Convert a TGS file to GIF with transparency support."""
        tgs_path = Path(tgs_path)
        
        if not tgs_path.exists():
            raise FileNotFoundError(f"TGS file not found: {tgs_path}")
        
        if output_path is None:
            output_path = tgs_path.with_suffix('.gif')
        else:
            output_path = Path(output_path)
        
        print(f"Converting TGS {tgs_path.name} to GIF...")
        
        try:
            # Parse TGS file
            animation = parse_tgs(str(tgs_path))
            
            # Export to GIF with transparency
            export_gif(animation, str(output_path))
            
            print(f"Successfully converted: {output_path.name}")
            return output_path
            
        except Exception as e:
            print(f"Error converting {tgs_path}: {str(e)}")
            raise


class WebmDecoder:
    """Decode WebM files with automatic transparency detection."""
    
    def __init__(self, filepath: str):
        self.filepath = filepath
        self._info = None
        if not os.path.exists(filepath):
            raise FileNotFoundError(f"File not found: {filepath}")
    
    def get_info(self) -> Dict[str, Any]:
        if self._info is not None:
            return self._info
        
        cmd = [
            'ffprobe',
            '-v', 'error',
            '-select_streams', 'v:0',
            '-show_entries', 
            'stream=width,height,r_frame_rate,duration,nb_frames,pix_fmt:format=duration',
            '-of', 'json',
            self.filepath
        ]
        
        try:
            result = subprocess.run(cmd, capture_output=True, text=True, check=True)
            data = json.loads(result.stdout)
        except subprocess.CalledProcessError as e:
            raise RuntimeError(f"ffprobe failed: {e.stderr}")
        except json.JSONDecodeError:
            raise RuntimeError("Failed to parse ffprobe output")
        
        if 'streams' not in data or not data['streams']:
            raise RuntimeError("No video stream found in file")
        
        stream = data['streams'][0]
        
        # Parse frame rate
        fps_str = stream.get('r_frame_rate', '30/1')
        if '/' in fps_str:
            num, den = map(int, fps_str.split('/'))
            fps = num / den if den != 0 else 30.0
        else:
            fps = float(fps_str)
        
        # Get duration
        duration = None
        if 'format' in data and 'duration' in data['format']:
            duration = float(data['format']['duration'])
        elif 'duration' in stream:
            duration = float(stream['duration'])
        
        if duration is None or duration <= 0:
            nb_frames = stream.get('nb_frames')
            if nb_frames:
                duration = int(nb_frames) / fps
            else:
                duration = 10.0
        
        # Check for alpha channel
        pix_fmt = stream.get('pix_fmt', '')
        has_alpha = 'yuva' in pix_fmt or 'rgba' in pix_fmt or 'argb' in pix_fmt
        
        self._info = {
            'width': int(stream['width']),
            'height': int(stream['height']),
            'fps': fps,
            'duration': duration,
            'nb_frames': int(stream.get('nb_frames', 0)),
            'has_alpha': has_alpha,
            'pix_fmt': pix_fmt
        }
        
        return self._info
    
    def analyze_black_pixels(self, sample_frames: int = 10) -> float:
        """Analyze black pixel ratio in video frames."""
        info = self.get_info()
        width = info['width']
        height = info['height']
        
        # Sample frames for analysis
        cmd = [
            'ffmpeg',
            '-i', self.filepath,
            '-vf', f'select=not(mod(n\\,{max(1, info["nb_frames"]//sample_frames)})),scale={min(320, width)}:{min(240, height)}',
            '-frames:v', str(sample_frames),
            '-f', 'rawvideo',
            '-pix_fmt', 'rgb24',
            '-'
        ]
        
        process = subprocess.run(cmd, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL)
        if process.returncode != 0:
            return 0.0
        
        # Analyze black pixels
        total_black = 0
        total_pixels = 0
        frame_size = min(320, width) * min(240, height) * 3
        
        data = process.stdout
        for i in range(0, len(data), frame_size):
            frame_data = data[i:i+frame_size]
            if len(frame_data) < frame_size:
                break
            
            frame = np.frombuffer(frame_data, dtype=np.uint8)
            # Check for near-black pixels (RGB < 20)
            black_pixels = np.sum(np.all(frame.reshape(-1, 3) < 20, axis=1))
            total_black += black_pixels
            total_pixels += len(frame) // 3
        
        return total_black / total_pixels if total_pixels > 0 else 0.0
    
    def decode_frames(self, use_alpha: bool = False) -> Iterator[Tuple[np.ndarray, float]]:
        info = self.get_info()
        width = info['width']
        height = info['height']
        
        # Choose pixel format based on alpha requirements
        if info['has_alpha'] or use_alpha:
            pix_fmt = 'rgba'
            channels = 4
        else:
            pix_fmt = 'rgb24'
            channels = 3
        
        cmd = [
            'ffmpeg',
            '-i', self.filepath,
            '-f', 'rawvideo',
            '-pix_fmt', pix_fmt,
            '-vcodec', 'rawvideo',
            '-'
        ]
        
        process = subprocess.Popen(cmd, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL)
        
        frame_size = width * height * channels
        frame_count = 0
        
        try:
            while True:
                raw_frame = process.stdout.read(frame_size)
                if len(raw_frame) != frame_size:
                    break
                
                frame = np.frombuffer(raw_frame, dtype=np.uint8)
                # Create writable copy
                frame = frame.reshape((height, width, channels)).copy()
                pts = frame_count / info['fps']
                
                yield frame, pts
                frame_count += 1
                
        finally:
            if process.poll() is None:
                process.terminate()
                try:
                    process.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait()


class GifWriter:
    """Write frames to GIF with automatic transparency handling."""
    
    def __init__(self, output_path: str, fps: float = 30.0, use_transparency: bool = False):
        self.output_path = output_path
        self.fps = fps
        self.use_transparency = use_transparency
        self.process = None
        self.frame_count = 0
        self._width = None
        self._height = None
    
    def _start_ffmpeg(self, width: int, height: int):
        self._width = width
        self._height = height
        
        if self.use_transparency:
            # Filter with transparency support
            filter_complex = (
                f"[0:v] fps={self.fps},scale={width}:{height}:flags=lanczos,split [a][b];"
                "[a] palettegen=reserve_transparent=1 [p];"
                "[b][p] paletteuse=alpha_threshold=128"
            )
            pix_fmt = 'rgba'
        else:
            # Standard filter
            filter_complex = (
                f"[0:v] fps={self.fps},scale={width}:{height}:flags=lanczos,split [a][b];"
                "[a] palettegen [p];"
                "[b][p] paletteuse"
            )
            pix_fmt = 'rgb24'
        
        cmd = [
            'ffmpeg',
            '-y',
            '-f', 'rawvideo',
            '-vcodec', 'rawvideo',
            '-pix_fmt', pix_fmt,
            '-s', f'{width}x{height}',
            '-r', str(self.fps),
            '-i', '-',
            '-filter_complex', filter_complex,
            '-loop', '0',
            self.output_path
        ]
        
        self.process = subprocess.Popen(
            cmd,
            stdin=subprocess.PIPE,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL
        )
    
    def add_frame(self, frame: np.ndarray):
        height, width = frame.shape[:2]
        
        if self.process is None:
            self._start_ffmpeg(width, height)
        
        # Ensure correct frame format
        if self.use_transparency:
            if frame.shape[2] == 3:
                # Add opaque alpha channel
                alpha = np.full((height, width, 1), 255, dtype=np.uint8)
                frame = np.concatenate([frame, alpha], axis=2)
        else:
            if frame.shape[2] == 4:
                # Remove alpha channel
                frame = frame[:, :, :3]
        
        if frame.dtype != np.uint8:
            frame = np.clip(frame, 0, 255).astype(np.uint8)
        
        try:
            self.process.stdin.write(frame.tobytes())
            self.process.stdin.flush()
            self.frame_count += 1
        except BrokenPipeError:
            if self.process.poll() is not None:
                raise RuntimeError("FFmpeg process died")
            raise
    
    def close(self):
        if self.process is not None:
            self.process.stdin.close()
            self.process.wait()
            
            if self.process.returncode != 0:
                raise RuntimeError(f"FFmpeg failed with code {self.process.returncode}")
            
            self.process = None
    
    def __enter__(self):
        return self
    
    def __exit__(self, exc_type, exc_val, exc_tb):
        self.close()


def convert_black_to_transparent(frame: np.ndarray, threshold: int = 16) -> np.ndarray:
    """Convert black pixels to transparent."""
    # Ensure frame is writable
    frame = np.array(frame, copy=True)
    
    if frame.shape[2] == 3:
        # Add alpha channel
        alpha = np.ones((frame.shape[0], frame.shape[1], 1), dtype=np.uint8) * 255
        frame = np.concatenate([frame, alpha], axis=2)
    
    # Detect black pixels
    black_mask = np.all(frame[:, :, :3] <= threshold, axis=2)
    
    # Make black pixels transparent
    frame[black_mask, 3] = 0
    
    return frame


def convert_webm_to_gif(webm_path: Path, gif_path: Path = None) -> Path:
    """Convert WebM to GIF with automatic transparency detection."""
    if gif_path is None:
        gif_path = webm_path.with_suffix('.gif')
    
    start_time = time.time()
    
    try:
        # Create decoder
        decoder = WebmDecoder(str(webm_path))
        info = decoder.get_info()
        
        # Auto-detect transparency requirements
        use_transparency = False
        convert_black = False
        
        if info['has_alpha']:
            # WebM has alpha channel, use it directly
            use_transparency = True
            print(f"Detected alpha channel ({info['pix_fmt']})")
        else:
            # Analyze for black pixels (might need transparency)
            print("Analyzing black pixels...", end='', flush=True)
            black_ratio = decoder.analyze_black_pixels()
            print(f"\r\033[K", end='')  # Clear line
            
            if black_ratio > 0.1:  # If more than 10% is black
                use_transparency = True
                convert_black = True
                print(f"Detected {black_ratio:.1%} black pixels, converting to transparent")
        
        print(f"Converting WebM {webm_path.name} to GIF...")
        
        # Create progress bar
        estimated_frames = int(info['duration'] * info['fps'])
        if estimated_frames <= 0:
            estimated_frames = 100
        progress = ProgressBar(total=estimated_frames, prefix=webm_path.name)
        
        # Write GIF
        writer = GifWriter(str(gif_path), fps=info['fps'], use_transparency=use_transparency)
        
        frame_count = 0
        try:
            for frame, pts in decoder.decode_frames(use_alpha=convert_black):
                # Convert black to transparent if needed
                if convert_black:
                    frame = convert_black_to_transparent(frame, threshold=16)
                
                writer.add_frame(frame)
                progress.update(1)
                frame_count += 1
        except Exception as frame_error:
            raise RuntimeError(f"Frame processing error at frame {frame_count}: {frame_error}")
        
        writer.close()
        progress.close()
        
        # Clear line and print completion info
        print(f"\r\033[K", end="")
        
        elapsed = time.time() - start_time
        file_size = gif_path.stat().st_size
        
        print(f"Finished \033[96m\033[1m{gif_path.name}\033[0m in {elapsed:.0f}s, "
              f"{humanize.naturalsize(file_size, binary=True)}")
        
        return gif_path
        
    except Exception as e:
        print(f"\nError processing {webm_path.name}: {e}")
        if gif_path.exists():
            gif_path.unlink()
        raise


def find_files_to_convert(directory: Path = Path("."), file_extensions: List[str] = None) -> List[Tuple[Path, Path]]:
    """Find all files that need conversion."""
    if file_extensions is None:
        file_extensions = ['.webm', '.tgs']
    
    files = []
    
    for entry in directory.iterdir():
        if entry.is_file() and entry.suffix.lower() in file_extensions:
            if entry.is_symlink():
                entry = entry.resolve()
                if not entry.is_file():
                    continue
            
            gif_path = entry.with_suffix(".gif")
            if not (gif_path.exists() and gif_path.stat().st_size > 0):
                files.append((entry, gif_path))
    
    return sorted(files, key=lambda x: x[0].name)


def process_single_file(input_path: Path, output_path: Path = None) -> bool:
    """Process a single file based on its extension."""
    if output_path is None:
        output_path = input_path.with_suffix('.gif')
    
    extension = input_path.suffix.lower()
    
    try:
        if extension == '.tgs':
            TgsConverter.convert_tgs_to_gif(input_path, output_path)
        elif extension == '.webm':
            convert_webm_to_gif(input_path, output_path)
        else:
            print(f"Unsupported file type: {extension}")
            return False
        return True
    except Exception as e:
        print(f"Error processing {input_path.name}: {e}")
        return False


def main():
    """Main entry point."""
    parser = argparse.ArgumentParser(
        description='Convert TGS (Telegram Sticker) and WebM files to GIF format with transparency support'
    )
    
    parser.add_argument(
        'input',
        nargs='*',
        help='Input files or directories to convert (default: current directory)'
    )
    
    parser.add_argument(
        '-o', '--output',
        help='Output GIF file path (only for single file conversion)'
    )
    
    parser.add_argument(
        '--tgs-only',
        action='store_true',
        help='Only process TGS files'
    )
    
    parser.add_argument(
        '--webm-only',
        action='store_true',
        help='Only process WebM files'
    )
    
    args = parser.parse_args()
    
    # Determine file extensions to process
    if args.tgs_only:
        file_extensions = ['.tgs']
    elif args.webm_only:
        file_extensions = ['.webm']
    else:
        file_extensions = ['.tgs', '.webm']
    
    if not args.input:
        # Process current directory
        files = find_files_to_convert(file_extensions=file_extensions)
        
        if not files:
            print("No files found to convert in current directory")
            return 0
        
        print(f"Found {len(files)} files to convert")
        
        # Process all files
        successful = 0
        for input_path, output_path in files:
            if process_single_file(input_path, output_path):
                successful += 1
        
        print(f"\nConversion complete! Successfully converted {successful}/{len(files)} files")
        
    else:
        # Process specified files
        if len(args.input) == 1 and args.output:
            # Single file with custom output
            input_path = Path(args.input[0])
            output_path = Path(args.output)
            
            if not input_path.exists():
                print(f"Error: Input file {input_path} does not exist")
                return 1
            
            if process_single_file(input_path, output_path):
                return 0
            else:
                return 1
        else:
            # Multiple files or single file with default output
            successful = 0
            total = len(args.input)
            
            for input_arg in args.input:
                input_path = Path(input_arg)
                
                if not input_path.exists():
                    print(f"Error: Input file {input_path} does not exist")
                    continue
                
                if input_path.is_dir():
                    # Process directory
                    dir_files = find_files_to_convert(input_path, file_extensions)
                    print(f"Found {len(dir_files)} files in {input_path}")
                    
                    for file_input, file_output in dir_files:
                        if process_single_file(file_input, file_output):
                            successful += 1
                else:
                    # Process single file
                    if process_single_file(input_path):
                        successful += 1
            
            print(f"\nConversion complete! Successfully converted {successful} files")
    
    return 0


if __name__ == '__main__':
    sys.exit(main())