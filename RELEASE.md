# Cutting a release

Summerpoem ships as a single self-contained `sump.exe`. Because GPU mining
compiles a CUDA kernel at build time, **release binaries must be built on a
machine with the CUDA toolkit installed** — a CI runner without CUDA produces
a CPU-only binary (GPU mining silently disabled). Builds are reproducible:
`Cargo.lock` is committed, pinning exact dependency versions.

## Build (Windows x64, GPU-capable, standalone)

Requirements: Rust (MSVC toolchain), the CUDA toolkit (`nvcc`), and MSVC build
tools. Then, with the CUDA `bin\x64` on PATH and `CUDA_PATH` set:

```powershell
$env:RUSTFLAGS = "-C target-feature=+crt-static"   # standalone: no VC++ runtime dependency
cargo build --release -p sump
```

The `crt-static` flag statically links the C runtime so the exe runs on a bare
Windows 10/11 machine with no redistributable. Confirm the only DLL
dependencies are Windows system libraries (no `vcruntime140.dll`):

```powershell
dumpbin /dependents target\release\sump.exe
```

## Package

```powershell
$dist = "dist"
Copy-Item target\release\sump.exe "$dist\sump.exe" -Force
Compress-Archive -Path "$dist\sump.exe","$dist\Start Mining.bat",`
  "$dist\Check Balance.bat","$dist\QUICKSTART.txt" `
  -DestinationPath "$dist\summerpoem-v<VERSION>-windows-x64.zip" -Force
Get-FileHash "$dist\summerpoem-v<VERSION>-windows-x64.zip" -Algorithm SHA256
```

The `.bat` launchers give non-technical users a double-click experience
(`sump.exe` is a CLI — double-clicking it alone just flashes a console).
They invoke the exe by absolute path (`%~dp0sump.exe`) so they work even
where the current directory is not on the executable search path.

## Publish (GitHub Release)

1. Tag the release commit: `git tag v<VERSION> && git push origin v<VERSION>`.
2. Create a Release for that tag (GitHub UI or `gh release create`).
3. Upload the `.zip` as a release asset.
4. In the release notes, publish:
   - the SHA-256 of the zip (so downloaders can verify integrity), and
   - the mainnet genesis hash
     (`60235b421eb3478072192851a1ea05eeb221dd8821aeaacb3fcd361abb21ca0d`),
     so anyone can confirm their node built the same genesis.

The landing page's Download button points at
`https://github.com/ardahash/SummerpoemCli/releases/latest`, which always
resolves to the newest release.

## Reproducible-build note

Anyone can rebuild from a tagged commit with the command above and compare
their binary's behavior and the genesis hash against the published release —
the strongest assurance for a downloadable miner, and the reason the project
is open source.
