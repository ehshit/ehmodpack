from pathlib import Path
import re
import sys

root = Path(sys.argv[1]).resolve() if len(sys.argv) > 1 else Path(__file__).resolve().parents[1]
locales = root / "locales"
source = root / "src" / "locale.rs"
start = "const DISPLAY_NAMES: &[(&str, &str)] = &["
end = "];"

bundled = {"en-US"}
if locales.is_dir():
    for path in locales.iterdir():
        file = path / "cli.ftl"
        if not path.is_dir() or not file.is_file():
            continue
        if any(
            line.strip() and not line.lstrip().startswith("#")
            for line in file.read_text(encoding="utf-8", errors="replace").splitlines()
        ):
            bundled.add(path.name)

text = source.read_text(encoding="utf-8")
begin = text.index(start) + len(start)
finish = text.index(end, begin)
block = text[begin:finish]

pattern = re.compile(
    r'^(?P<indent>[ \t]*)(?P<comment>//[ \t]*)?\("(?P<code>[^"]+)",[ \t]*"(?P<name>[^"]+)"\),$',
    re.MULTILINE,
)


def update(match):
    code = match.group("code")
    marker = "" if code in bundled else "// "
    indent = match.group("indent")
    name = match.group("name")
    return f'{indent}{marker}("{code}", "{name}"),'


updated_block = pattern.sub(update, block)
if updated_block != block:
    source.write_text(text[:begin] + updated_block + text[finish:], encoding="utf-8")

enabled = sorted(code for code in bundled if code != "en-US")
print("enabled locales: " + (", ".join(enabled) if enabled else "none"))