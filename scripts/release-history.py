"""Collect published stable release notes for signed update metadata."""
import argparse
import json
import os
import re
import urllib.request
from pathlib import Path

REPOSITORY = "xwwpy/WW-StudyAPP"


def version_key(value):
    if not isinstance(value, str) or not re.fullmatch(r"(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)", value):
        raise ValueError("Expected a stable release version")
    return tuple(map(int, value.split(".")))


def published_history(pages, target):
    target_key = version_key(target)
    releases = {}
    for page in pages:
        if not isinstance(page, list):
            raise ValueError("Invalid release listing")
        for release in page:
            tag = release.get("tag_name", "")
            if release.get("draft") or release.get("prerelease") or not tag.startswith("v"):
                continue
            try:
                key = version_key(tag[1:])
            except ValueError:
                continue
            if key >= target_key:
                continue
            if tag in releases:
                raise ValueError("Duplicate published release")
            notes = release.get("body") or "本版本未提供更新说明。"
            if not isinstance(notes, str) or not isinstance(release.get("published_at"), str):
                raise ValueError("Invalid published release notes")
            releases[tag] = {"version": tag[1:], "notes": notes.strip(), "pub_date": release["published_at"]}
    return sorted(releases.values(), key=lambda row: version_key(row["version"]), reverse=True)


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, *args, **kwargs):
        return None


def fetch_history(target):
    pages = []
    headers = {"Accept": "application/vnd.github+json", "User-Agent": "WW-StudyApp-release"}
    token = os.environ.get("GH_TOKEN")
    if token:
        headers["Authorization"] = f"Bearer {token}"
    for page in range(1, 101):
        url = f"https://api.github.com/repos/{REPOSITORY}/releases?per_page=100&page={page}"
        request = urllib.request.Request(url, headers=headers)
        with urllib.request.build_opener(NoRedirect).open(request, timeout=60) as response:
            data = response.read(4 * 1024 * 1024 + 1)
        if len(data) > 4 * 1024 * 1024:
            raise ValueError("Release listing too large")
        rows = json.loads(data)
        if not isinstance(rows, list):
            raise ValueError("Invalid release listing")
        pages.append(rows)
        if len(rows) < 100:
            return published_history(pages, target)
    raise ValueError("Pagination exceeded; refusing incomplete history")


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--target-version", required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    args.output.write_text(json.dumps(fetch_history(args.target_version), ensure_ascii=False) + "\n", encoding="utf-8")


if __name__ == "__main__":
    main()
