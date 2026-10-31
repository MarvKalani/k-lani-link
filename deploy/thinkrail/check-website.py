"""Check the exported static demo's file references, anchors, asset integrity and lack of external resources."""
from html.parser import HTMLParser
import argparse
import hashlib
from pathlib import Path
import re

ROOT = Path(__file__).resolve().parents[2]


class Page(HTMLParser):
    def __init__(self):
        super().__init__()
        self.ids, self.references, self.images = [], [], []
        self.language = None

    def handle_starttag(self, tag, attributes):
        values = dict(attributes)
        if tag == "html":
            self.language = values.get("lang")
        if "id" in values:
            self.ids.append(values["id"])
        if tag == "img":
            self.images.append(values)
        for key in ("href", "src"):
            if values.get(key):
                self.references.append(values[key])


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--directory", type=Path, default=ROOT / "website")
    parser.add_argument("--language", default="en")
    args = parser.parse_args()
    site = args.directory.resolve(strict=True)
    page = Page()
    page.feed((site / "index.html").read_text())
    assert page.language == args.language
    assert len(page.ids) == len(set(page.ids)), "duplicate element IDs"
    for value in page.references:
        if value.startswith("#"):
            assert value[1:] in page.ids, "missing internal anchor"
        elif value.startswith("data:image/"):
            continue
        else:
            assert not re.match(r"(?:[a-z]+:|//)", value, re.I), "external resource or link"
            path = (site / value).resolve(strict=True)
            assert path.is_relative_to(site.resolve()) and path.is_file()
    assert any(i.get("src") == "assets/k-lani-link.webp" and i.get("alt") for i in page.images)
    assert hashlib.sha256((site / "assets/k-lani-link.webp").read_bytes()).digest() == hashlib.sha256((ROOT / "assets/k-lani-link.webp").read_bytes()).digest()
    for path in site.glob("*.js"):
        script = path.read_text()
        assert not re.search(r"\b(?:fetch|WebSocket|XMLHttpRequest|EventSource|eval)\s*\(", script)
    css = (site / "styles.css").read_text()
    css = re.sub(r"/\*.*?\*/", "", css, flags=re.S)
    assert "@import" not in css and "@font-face" not in css
    assert not re.search(r"url\(\s*['\"]?(?:https?:|//)", css)
    print("PASS: local resources, internal anchors, requested language, project image and static network checks.")


if __name__ == "__main__":
    main()
