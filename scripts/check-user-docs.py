"""Keep the public documentation and packaged guides focused on product usage."""
from pathlib import Path
import re
import subprocess

ROOT = Path(__file__).resolve().parent.parent
GUIDES = {
    "user-guide.md", "storage.md", "sync-guide.md", "object-storage.md",
    "updates.md", "release-notes.md",
}
IMAGES = {"dashboard.png", "managed-images.png", "note-editor.png", "sync-settings.png", "workspace.png"}
allowed = {f"docs/{name}" for name in GUIDES} | {f"docs/images/{name}" for name in IMAGES}
tracked = set(subprocess.check_output(
    ["git", "ls-files", "docs"], cwd=ROOT, text=True, encoding="utf-8"
).splitlines())
unexpected = tracked - allowed
assert not unexpected, f"Only usage guides may be published under docs: {sorted(unexpected)}"
for name in GUIDES:
    path = ROOT / "docs" / name
    text = path.read_text(encoding="utf-8-sig")
    for target in re.findall(r"\]\(([^)]+)\)", text):
        if "://" in target or target.startswith("#"):
            continue
        resolved = (path.parent / target.split("#")[0]).resolve()
        assert resolved.is_relative_to(ROOT), f"External local link in {name}"
        assert resolved.relative_to(ROOT).as_posix() in allowed, f"Non-public documentation link in {name}: {target}"
        assert resolved.is_file(), f"Missing documentation link in {name}: {target}"
    assert not re.search(r"(?i)\b(?:SKILL\.md|AGENTS\.md|codex/|superpowers|TODO|fixture)\b|提示词|开发过程|验收记录|实现计划", text), f"Internal material in {name}"
print(f"Public documentation: {len(GUIDES)} usage guides; links and publication boundaries verified.")
