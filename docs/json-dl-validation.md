# json-dl local validation

Date: 2026-10-19. This records local validation before the first CI run; it does not claim remote builds passed.

## Passed checks

- 12 libdl unit tests, including upstream range, resume, and rate-limit fallback tests.
- 19 black-box subprocess integration tests on each of the four release binaries below.
- `cargo check --locked -p dl`, with upstream torrent dependencies enabled.
- `cargo clippy --locked -p json-dl --no-deps -- -D warnings`.
- Native Windows HTTPS download of the fork LICENSE, checked against the original LF byte size and SHA-256.
- YAML parsing and the eight-target build matrix; GitHub workflow permissions are read-only.
- All four distribution archive hashes checked against their metadata.

| Built target | Runtime tests | Archive SHA-256 |
|---|---|---|
| `aarch64-unknown-linux-gnu` | QEMU ARM64 user mode; 19/19 | `48d658249483937a1ba55ad9bed8bcd664f1629031e2bcc69e272bf8c94dca15` |
| `x86_64-pc-windows-gnu` | Native Windows; 19/19 | `08407a17b75bf60cb19c4a1f8cbd08f1e5fa960dfd7f764d5d560baea3c98aae` |
| `x86_64-unknown-linux-gnu` | Docker Linux x86_64; 19/19 | `c340f21290e10b42e3cb9a55ef598fb1aa0767a44d720b79500630a568a77697` |
| `x86_64-unknown-linux-musl` | Docker Linux x86_64; 19/19 | `c6b7a2760c04cb4458578ec2336fb02fc6353aebf98926fc4b859254fa364bda` |

Archives are in the Git-ignored `dist/` directory. Packages contain the binary, license, protocol guide, checksums, and binary/archive metadata. A native Windows copy is at `dist/native-windows/json-dl.exe`.

## Toolchain and limits

- Official `rust:1.96-bookworm` image, Rust 1.96; image digest `sha256:a339861ae23e9abb272cea45dfafde21760d2ce6577a70f8a926153677902663`.
- Debian cross GCC, MinGW, musl-gcc; Python 3.12.10 on Windows and Python stdlib in the container.
- The local GNU/Linux x86_64 and ARM64 binaries require symbols up to GLIBC_2.34, plus system libgcc/libm/libc. Minimum-distribution runtime acceptance has not been performed.
- The Linux x86_64 musl binary is static PIE and does not depend on glibc. Distribution-specific DNS, certificates, and filesystems still require acceptance testing.
- ARM64 tests used QEMU, not ARM64 hardware.
- The Windows local binary is GNU/MinGW. It does not replace native MSVC x86_64/ARM64 CI results.
- No real muapack credentials, OSS/CDN load test, package signature verification, or installation acceptance was performed.

## Remote build plan

`.github/workflows/json-dl.yml` builds Windows MSVC x86_64/ARM64, Linux GNU and musl x86_64/ARM64, and macOS Intel/Apple Silicon. A separate job merges the macOS binaries into a Universal Binary with ad-hoc signing. Developer ID signing and notarization are not configured.

References: [Rust targets](https://doc.rust-lang.org/rustc/platform-support.html), [GitHub runner labels](https://docs.github.com/en/actions/reference/runners/github-hosted-runners).
