"""Check the static site's local resources, approved navigation, anchors and asset integrity."""
from html.parser import HTMLParser
import argparse
import hashlib
from pathlib import Path
import re
from urllib.parse import urlsplit

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
                self.references.append((tag, key, values[key]))


def allowed_navigation(tag, key, value):
    """Allow deliberate product/checkout navigation, never remote page resources."""
    if tag != "a" or key != "href":
        return False
    if value == "https://buy.stripe.com/3cIcN4gDC06MeWW9p7ew802":
        return True
    parsed = urlsplit(value)
    project = "/MarvKalani/k-lani-link"
    return (
        parsed.scheme == "https"
        and parsed.netloc == "github.com"
        and (parsed.path == project or parsed.path.startswith(project + "/"))
    )


def validate_reference(tag, key, value, site, ids):
    if value.startswith("#"):
        assert value[1:] in ids, "missing internal anchor"
    elif tag == "img" and key == "src" and value.startswith("data:image/"):
        return
    elif allowed_navigation(tag, key, value):
        return
    else:
        assert not re.match(r"(?:[a-z]+:|//)", value, re.I), "external resource or unapproved navigation"
        path = (site / value).resolve(strict=True)
        assert path.is_relative_to(site.resolve()) and path.is_file()


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
    for tag, key, value in page.references:
        validate_reference(tag, key, value, site, page.ids)
    assert any(i.get("src") == "assets/k-lani-link.webp" and i.get("alt") for i in page.images)
    assert hashlib.sha256((site / "assets/k-lani-link.webp").read_bytes()).digest() == hashlib.sha256((ROOT / "assets/k-lani-link.webp").read_bytes()).digest()
    for path in site.glob("*.js"):
        script = path.read_text()
        assert not re.search(r"\b(?:fetch|WebSocket|XMLHttpRequest|EventSource|eval)\s*\(", script)
    css = (site / "styles.css").read_text()
    css = re.sub(r"/\*.*?\*/", "", css, flags=re.S)
    assert "@import" not in css and "@font-face" not in css
    assert not re.search(r"url\(\s*['\"]?(?:https?:|//)", css)
    print("PASS: local resources, approved navigation, internal anchors, language, artwork and static network checks.")


if __name__ == "__main__":
    main()
