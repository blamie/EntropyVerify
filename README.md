# EntropyVerify (EVFY)

**EntropyVerify** is a high-performance, open-source storage validation utility. It serves as an ultra-fast alternative to legacy tools like H2testw, written in Rust and optimized specifically for modern high-speed PCIe 4.0/5.0 NVMe SSDs but it works with any drive.

The primary goal  of EntropyVerify is to provide absolute accuracy in verifying real storage capacity while safely maximizing the physical read/write throughput of high-speed storage.
<img width="1115" height="628" alt="Screenshot 2026-06-23 012456" src="https://github.com/user-attachments/assets/cf548a6e-4206-40ce-af28-0dab3ef5d724" />

---

## Key Features

* **High-Performance Asynchronous I/O Engines**:
  * **Windows**: Built on Win32 Overlapped I/O with **I/O Completion Ports (IOCP)** and direct hardware access via `FILE_FLAG_NO_BUFFERING` and `FILE_FLAG_WRITE_THROUGH` (Direct I/O).
  * **Linux**: Optimized utilizing raw **`io_uring`** with `O_DIRECT`, with a clean synchronous `pread`/`pwrite` fallback loop for older kernels (< 5.6).
* **Hardware-Accelerated Cryptography**:
  * **Data Generation**: Uses **AES-128-CTR** stream generation. Nonces are deterministically derived from block and file indices, allowing on-the-fly verification without storing expectations. Key schedules are hardware-accelerated via **AES-NI** intrinsics, achieving >7 GB/s single-core generator throughput.
  * **Block Checksums**: Employs **BLAKE3** block-level hashing (vectorized via AVX2/SSE4.1) for secure, deterministic integrity verification.
* **Low-Level Safety Guardrails**:
  * Blocks execution targeting the OS system partition (`C:\` on Windows, `/` on Linux) directly at the root, protecting system drives from accidental overwrites.
  * Blacklists critical system mount points on Linux (`/boot`, `/etc`, `/usr`, `/var`, `/proc`, `/sys`, `/dev` and active swap partitions).
  * Performs active write/remove sentinel checks to guarantee target directory write permissions before writing test files.
* **Core-Pinned Architecture**:
  * Detects physical CPU topology and assigns worker threads to dedicated physical cores, leaving Core 0 open for terminal interrupts and the TUI dashboard to eliminate UI stuttering.
* **Interactive TUI Dashboard**:
  * Renders a real-time TUI dashboard using `ratatui` with rolling throughput sparklines, detailed phase progress bars, worker queue stats, error logs, and real-time ETAs.
  * Interactive keys: `[Tab]` to switch byte units (GB/TB vs GiB/TiB), `[P]` to pause/resume worker execution via cross-thread `Condvar` synchronization, and `[Q]` to gracefully abort and sync changes.
* **Smart Double-Click Detection**:
  * When run without arguments (e.g., double-clicking the `.exe` in Windows Explorer), the tool automatically falls back to an interactive prompt mode asking for the target directory, and holds the command prompt open upon exit (`Press Enter to exit...`) so results can be reviewed.
* **Persistent Markdown Reports**:
  * Generates a persistent verification report `entropy_verify_report_YYYYMMDD_HHMMSS.md` on the target volume containing filesystem information, comprehensive phase speeds, peak transfer rates, and precise block corruption offset logs.
  * **Auto-Cleanup**: Automatically cleans up all chunk files (`ev_chunk_*.bin`) upon a 100% successful validation pass, but retains them on block mismatch to enable analysis.

---

## Block Format Specification

EntropyVerify segments target volumes into individual files (default: 1 GiB) consisting of consecutive **2 MiB blocks**. Direct I/O requires page-aligned buffers, which is managed using a custom `AlignedBuffer` allocating virtual pages aligned to 4096-byte boundaries (`VirtualAlloc` on Windows / `posix_memalign` on Linux).

Each 2 MiB block has the following binary layout:

| Offset (Bytes) | Size (Bytes) | Field Name | Type | Description |
|---|---|---|---|---|
| `0x00` | 4 | Magic | `[u8; 4]` | Hardcoded signature `b"EVFY"` |
| `0x04` | 1 | Version | `u8` | Header format version (currently `1`) |
| `0x05` | 3 | Padding | `[u8; 3]` | Alignment padding |
| `0x08` | 4 | File Index | `u32` | Index of the parent file chunk |
| `0x0C` | 4 | Block Index | `u32` | Offset index of the block within the file |
| `0x10` | 2 | Thread ID | `u16` | ID of the worker thread that wrote the block |
| `0x12` | 2 | Padding | `[u8; 2]` | Alignment padding |
| `0x14` | 16 | AES Nonce | `[u8; 16]` | Deterministic AES-CTR block nonce |
| `0x24` | 32 | Checksum | `[u8; 32]` | BLAKE3 hash of the payload section |
| `0x44` | 2,097,084 | Payload | `[u8]` | Deterministic AES-CTR pseudo-random stream |

---

## Getting Started

### Prerequisites

* Rust Toolchain (v1.75 or later recommended)
* A target CPU supporting **AES-NI** instructions (enforced globally at compile time via target-features for x86_64 architectures).

### Building from Source

To build a release binary:

```bash
# Clone the repository
git clone https://github.com/blamie/EntropyVerify.git
cd EntropyVerify/entropy_verify

# Compile in release mode
cargo build --release
```

The compiled binary will be located at:
* **Windows**: `target/release/entropy_verify.exe`
* **Linux**: `target/release/entropy_verify`

*Note: The build process configures `.cargo/config.toml` targeting `+aes` and `+sse4.1` for maximum single-core block generation speed.*

---

## Usage Guide

### Terminal CLI Options

You can invoke the CLI from your terminal with custom parameters:

```bash
target/release/entropy_verify --target-dir E:\test_folder
```

#### Available Command-Line Arguments:

* `-t, --target-dir <PATH>`: *(Required in CLI mode)* Path to the directory on the volume you wish to test.
* `--block-size <BYTES>`: I/O block size in bytes (default: `2097152` / 2 MiB). Must be a multiple of the drive sector size (typically 512 or 4096).
* `--file-size <BYTES>`: File chunk segment size in bytes (default: `1073741824` / 1 GiB).
* `--queue-depth <N>`: Async queue depth depth per worker thread (default: `32`).
* `--threads <N>`: Number of concurrent worker threads. Defaults to the number of physical CPU cores minus 1 (reserving Core 0 for TUI rendering).
* `--write-only`: Runs only the writing phase, creating files and exiting without verification.
* `--verify-only`: Bypasses the write phase and runs verification on existing `ev_chunk_*.bin` files in the directory.
* `--seed <HEX>`: Custom 16-byte master key as hex string (e.g., `0xDEADBEEF...`). If omitted, a random cryptographic key is generated.

### Interactive/Double-Click Mode

If you run the binary by double-clicking it on Windows (no command-line arguments provided):
1. A console window will open.
2. You will be prompted to type in your target path:
   ```
   EntropyVerify — Storage Validation Utility
   -------------------------------------------
   Enter the target directory to test (e.g., E:\):
   ```
3. Type the drive letter or folder path (e.g., `D:\test`) and hit **Enter**.
4. The tool will calculate the maximum usable space (leaving a 100 MiB safety margin so the target drive doesn't become completely full) and initiate the TUI dashboard.
5. Upon completion or failure, the terminal will wait for your interaction:
   ```
   Press Enter to exit...
   ```
   This ensures the console window doesn't disappear before you review the test summary.

### In-Test Hotkeys

While the TUI dashboard is active, you can press the following keys:
* **`[Tab]`**: Dynamically toggle capacity units between decimal (`GB`/`TB` base-1000) and binary (`GiB`/`TiB` base-1024) formats.
* **`[P]`**: Pause or resume worker thread operations. Helpful if you need to temporarily reduce system I/O load.
* **`[Q]`**: Gracefully terminate testing. This signals the threads to complete their current block, flushes file buffers, closes open file handles, restores your terminal layout, and saves an interrupted markdown report.

---

## Verification Reports

On completion, verification reports are saved directly to the target volume. An example report structure:

```markdown
# EntropyVerify Report

**Date:** 2026-06-22 18:30:15
**Tool Version:** 1.0.0
**Status:** ✅ **PASSED** — All blocks verified successfully.

## Drive Information
| Property | Value |
|---|---|
| Path | `E:\test_folder` |
| Filesystem | NTFS |
| Volume Label | BackupDrive |
| Reported Capacity | 1,000,204,886,016 bytes (1.00 TB) |
| Tested Capacity | 1,000,104,886,016 bytes (1.00 TB) |

## Performance Summary
| Metric | Write Phase | Verify Phase |
|---|---|---|
| Duration | 00:02:35 | 00:01:58 |
| Throughput (avg) | 6,452.3 MB/s | 7,120.8 MB/s |
| Peak Throughput | 6,891.5 MB/s | 7,340.2 MB/s |
| Total Data | 1.00 TB | 1.00 TB |

## Configuration
| Parameter | Value |
|---|---|
| Block Size | 2.0 MiB |
| File Size | 1.0 GiB |
| Queue Depth | 32 |
| Worker Threads | 8 |
| AES-CTR Seed | `0x5A7D8F9E0102030405060708090A0B0C` |

## Corrupted Blocks
> None detected. ✅

---
*Generated by [EntropyVerify](https://github.com/blamie/EntropyVerify) v1.0.0*
```

*Note: In the event of errors or block mismatches, the report will display a tabular breakdown containing the file index, block index, expected BLAKE3 hash, and actual BLAKE3 hash of every corrupted block.*
