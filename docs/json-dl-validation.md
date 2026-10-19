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


## Completed remote validation

Source commit: `ea2ba687f0d7501f2aa99180c26aaa3714addd36`.

GitHub Actions: https://github.com/Sam-Chai/dl/actions/runs/39245761110

All eight native target jobs passed unit tests and 19 protocol/HTTP integration tests each. The upstream CLI compatibility check also passed. The macOS Universal job passed architecture and ad-hoc signature verification, followed by 19 integration tests on Apple Silicon. The Universal Intel slice was not separately executed after merging; its thin binary passed on the Intel runner.

All nine downloaded packages were checked locally against archive and executable SHA-256 metadata, platform/architecture headers, required documentation, and Unix executable permissions. Artifacts are stored in `dist/ci-39245761110/`.

| Target | Archive SHA-256 |
|---|---|
| `aarch64-apple-darwin` | `fa4889dc87e32401763ee3893b67e604521d328d6d3abfd7845616cb06248380` |
| `aarch64-pc-windows-msvc` | `fa3c4660b4afa3e9f5fdbe622481aa563e3cd82f8cff3aff57b5db309337273e` |
| `aarch64-unknown-linux-gnu` | `03f714375d1dc5a11d3e0c805b2d403ee782707ab7adb29e184a9345df5b7f29` |
| `aarch64-unknown-linux-musl` | `449a3351cba292577ae1efc77644e47f18a8ca7400f48d49a9d1789c311bc092` |
| `universal-apple-darwin` | `d74d8f3ae94b0f761357508c3de10b3c28b0f2d46646b1f81b9ead014a6e168b` |
| `x86_64-apple-darwin` | `82b18064530305a18866adc5aabdf2badfd40c435d024459d90eb6982e29d3c6` |
| `x86_64-pc-windows-msvc` | `9019f970c53f34aa74b30f9c057ce246eb11d5a3c89d31507e8e1f4db0110a65` |
| `x86_64-unknown-linux-gnu` | `6f1e065dabda7ec6ba179cb6d073100ed47d1b2746bf2501cb515a1ca423c347` |
| `x86_64-unknown-linux-musl` | `c81687cbaf347645003206145301681c1c7e3f356e4a52e8d8040f6578111d45` |
