"""Create a release archive containing only the executable and its documentation."""

import argparse
import tarfile
import tomllib
import zipfile
from pathlib import Path


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("platform", choices=["windows-x86_64", "linux-x86_64", "macos-arm64", "macos-x86_64"])
    parser.add_argument("binary", type=Path)
    parser.add_argument("--output", type=Path, default=Path("dist"))
    args = parser.parse_args()

    with Path("Cargo.toml").open("rb") as manifest:
        version = tomllib.load(manifest)["package"]["version"]
    root = f"kawaks-rs-v{version}-{args.platform}"
    exe = "kawaks-rs.exe" if args.platform.startswith("windows") else "kawaks-rs"
    files = {
        exe: args.binary,
        "README.md": Path("README.md"),
        "LICENSE": Path("LICENSE"),
        "THIRD_PARTY.md": Path("THIRD_PARTY.md"),
        "OFL-Noto.txt": Path("src/gui/OFL-Noto.txt"),
    }
    for path in files.values():
        if not path.is_file():
            parser.error(f"Missing release input: {path}")

    args.output.mkdir(parents=True, exist_ok=True)
    if exe.endswith(".exe"):
        artifact = args.output / f"{root}.zip"
        with zipfile.ZipFile(artifact, "w", compression=zipfile.ZIP_DEFLATED) as archive:
            for name, path in files.items():
                archive.write(path, f"{root}/{name}")
    else:
        if not args.binary.stat().st_mode & 0o111:
            parser.error(f"Release binary is not executable: {args.binary}")
        artifact = args.output / f"{root}.tar.gz"
        with tarfile.open(artifact, "w:gz") as archive:
            for name, path in files.items():
                archive.add(path, arcname=f"{root}/{name}", recursive=False)
    print(artifact)


if __name__ == "__main__":
    main()
