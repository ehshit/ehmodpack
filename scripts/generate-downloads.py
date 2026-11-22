import json
import os
from datetime import datetime, timezone
from pathlib import Path
from urllib.request import Request, urlopen

ARCHIVE_NAMES = [
    ("Windows x64", "windows-x86_64", "ehmodpack-windows-x86_64.zip"),
    ("macOS ARM64 (Apple Silicon)", "macos-aarch64", "ehmodpack-macos-aarch64.tar.gz"),
    ("macOS Intel", "macos-x86_64", "ehmodpack-macos-x86_64.tar.gz"),
    ("Linux x64", "linux-x86_64", "ehmodpack-linux-x86_64.tar.gz"),
    ("Linux ARM64", "linux-aarch64", "ehmodpack-linux-aarch64.tar.gz"),
    ("Linux ARMv7", "linux-armv7", "ehmodpack-linux-armv7.tar.gz"),
    ("Linux i686", "linux-i686", "ehmodpack-linux-i686.tar.gz"),
    ("Linux PowerPC64 LE", "linux-powerpc64le", "ehmodpack-linux-powerpc64le.tar.gz"),
    ("Linux RISC-V 64", "linux-riscv64gc", "ehmodpack-linux-riscv64gc.tar.gz"),
    ("Linux s390x", "linux-s390x", "ehmodpack-linux-s390x.tar.gz"),
]

PACKAGE_NAMES = [
    ("x64", "amd64", "x86_64"),
    ("ARM64", "arm64", "aarch64"),
    ("ARMv7", "armhf", "armv7hl"),
    ("i686", "i386", "i686"),
    ("PowerPC64 LE", "ppc64el", "ppc64le"),
    ("RISC-V 64", "riscv64", None),
    ("s390x", "s390x", "s390x"),
]


def fetch_json(url, token=None):
    headers = {
        "Accept": "application/vnd.github+json",
        "User-Agent": "ehmodpack-website-updater",
    }
    if token:
        headers["Authorization"] = f"Bearer {token}"
    request = Request(url, headers=headers)
    with urlopen(request, timeout=30) as response:
        return json.load(response)


repo = os.environ["REPOSITORY"]
tag = os.environ["RELEASE_TAG"]
version = tag.lstrip("v")
token = os.environ.get("GH_TOKEN")

release = fetch_json(f"https://api.github.com/repos/{repo}/releases/tags/{tag}", token)
assets = {asset["name"]: asset["browser_download_url"] for asset in release["assets"]}


def asset_url(name):
    if name not in assets:
        raise SystemExit(f"missing release asset: {name}")
    return assets[name]


winget = fetch_json("https://api.ehis.gay/v1/winget?package=Eh.Modpack")
data = {
    "version": version,
    "tag": tag,
    "generatedAt": datetime.now(timezone.utc).replace(microsecond=0).isoformat().replace("+00:00", "Z"),
    "winget": {
        "package": winget["identifier"],
        "version": winget.get("latestVersion") or winget.get("version") or version,
    },
    "archives": [
        {
            "system": system,
            "arch": arch,
            "file": file_name,
            "url": asset_url(file_name),
        }
        for system, arch, file_name in ARCHIVE_NAMES
    ],
    "linuxPackages": [
        {
            "system": system,
            "deb": asset_url(f"ehmodpack-{version}-{deb_arch}.deb"),
            "rpm": asset_url(f"ehmodpack-{version}-{rpm_arch}.rpm") if rpm_arch else None,
        }
        for system, deb_arch, rpm_arch in PACKAGE_NAMES
    ],
}

path = Path("website/src/data/downloads.json")
path.parent.mkdir(parents=True, exist_ok=True)
with open(path, "w", encoding="utf-8", newline="\n") as handle:
    json.dump(data, handle, indent=2, ensure_ascii=False)
    handle.write("\n")

print(f"wrote {path} for {tag}")
