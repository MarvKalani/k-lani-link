import importlib.util
from pathlib import Path
import tempfile
import unittest

spec = importlib.util.spec_from_file_location(
    "website_check", Path(__file__).with_name("check-website.py")
)
website = importlib.util.module_from_spec(spec)
spec.loader.exec_module(website)


class WebsiteReferences(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.site = Path(self.temp.name) / "site"
        self.site.mkdir()
        (self.site / "styles.css").write_text("body {}")
        (Path(self.temp.name) / "outside.txt").write_text("outside")

    def check(self, tag, key, value):
        website.validate_reference(tag, key, value, self.site, {"main"})

    def test_project_and_existing_checkout_links_are_navigation(self):
        for url in [
            "https://github.com/MarvKalani/k-lani-link",
            "https://github.com/MarvKalani/k-lani-link/releases",
            "https://github.com/MarvKalani/k-lani-link/blob/main/LICENSING.md",
            "https://buy.stripe.com/3cIcN4gDC06MeWW9p7ew802",
        ]:
            with self.subTest(url=url):
                self.check("a", "href", url)

    def test_approved_hosts_cannot_load_remote_resources(self):
        url = "https://github.com/MarvKalani/k-lani-link/remote.js"
        for tag, key in [("script", "src"), ("img", "src"), ("link", "href")]:
            with self.subTest(tag=tag):
                with self.assertRaises(AssertionError):
                    self.check(tag, key, url)

    def test_unapproved_and_ambiguous_destinations_are_rejected(self):
        for url in [
            "https://example.com",
            "http://github.com/MarvKalani/k-lani-link",
            "//github.com/MarvKalani/k-lani-link",
            "https://github.com.evil.example/MarvKalani/k-lani-link",
            "https://user@github.com/MarvKalani/k-lani-link",
            "https://github.com/MarvKalani/k-lani-link-other",
            "https://buy.stripe.com/another-checkout",
            "javascript:alert(1)",
        ]:
            with self.subTest(url=url):
                with self.assertRaises(AssertionError):
                    self.check("a", "href", url)

    def test_local_resources_and_known_anchors_are_kept(self):
        self.check("link", "href", "styles.css")
        self.check("a", "href", "#main")
        self.check("img", "src", "data:image/png;base64,example")
        with self.assertRaises(AssertionError):
            self.check("a", "href", "#missing")

    def test_page_resources_stay_inside_the_site_directory(self):
        with self.assertRaises(AssertionError):
            self.check("a", "href", "../outside.txt")
        with self.assertRaises(AssertionError):
            self.check("a", "href", str(Path(self.temp.name) / "outside.txt"))


if __name__ == "__main__":
    unittest.main()
