## Which file do I download?

| Your computer | Download |
|---|---|
| Windows, 64-bit (almost every Windows 10/11 PC) | `opencode-windows-64bit.zip` |
| Windows, 32-bit (older PCs) | `opencode-windows-32bit.zip` |
| Mac with Apple Silicon (M1, M2, M3, M4…) | `opencode-macos-apple-silicon.tar.gz` |
| Mac with an Intel processor | `opencode-macos-intel.tar.gz` |
| Linux, 64-bit Intel/AMD (most desktops, laptops and servers) | `opencode-linux-64bit.tar.gz` |
| Linux on ARM (Raspberry Pi 4/5, ARM servers, Android via Termux) | `opencode-linux-arm64.tar.gz` |

Not sure? On Windows open *Settings → System → About* and look at *System type*. On a Mac, open  → *About This Mac*: "Chip: Apple M…" means Apple Silicon. On Linux, run `uname -m`: `x86_64` → 64bit, `aarch64` → arm64.

Each archive contains the `opencode` program (`opencode.exe` on Windows) and the README. The `.sha256` files let you verify the download. See the [README](https://github.com/exgg1453/opencode-rs#installation) for installation steps.
