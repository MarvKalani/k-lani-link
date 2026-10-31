import importlib.util
from pathlib import Path
import unittest

spec = importlib.util.spec_from_file_location("download_links", Path(__file__).with_name("update_downloads.py"))
links = importlib.util.module_from_spec(spec)
spec.loader.exec_module(links)


def release(version="1.2610.29", date="2026-10-28T14:20:48Z", **flags):
    tag = "v" + version
    names = [f"k-lani-link-{version}-{suffix}" for suffix in links.SUFFIXES] + ["SHA256SUMS"]
    return {"tag_name": tag, "published_at": date, "draft": False, "prerelease": True,
            "assets": [{"name": name, "state": "uploaded", "size": 100,
                        "browser_download_url": f"{links.BASE}/releases/download/{tag}/{name}"} for name in names], **flags}


class DownloadLinks(unittest.TestCase):
    def test_latest_complete_preview_is_included_regardless_of_api_order(self):
        older = release(prerelease=False)
        newer = release("1.2610.30", "2026-10-30T12:00:00Z")
        for rows in ([older, newer], [newer, older]):
            self.assertEqual(links.newest_complete(rows), newer)

    def test_preview_revision_assets_are_selected_without_rewriting_the_version(self):
        current = release()
        newer = release("1.2610.29-preview.2", "2026-10-29T12:00:00Z")
        selected = links.newest_complete([current, newer])
        self.assertEqual(selected, newer)
        self.assertIn("/v1.2610.29-preview.2/k-lani-link-1.2610.29-preview.2-windows-x64.exe", links.block(selected))

    def test_drafts_partial_uploads_and_wrong_urls_never_replace_working_downloads(self):
        current = release()
        for change in ("draft", "missing", "uploading", "empty", "foreign-url", "invalid-date", "invalid-tag"):
            next_release = release("1.2610.30", "2026-10-30T12:00:00Z")
            if change == "draft":
                next_release["draft"] = True
            elif change == "missing":
                next_release["assets"].pop()
            elif change == "uploading":
                next_release["assets"][0]["state"] = "new"
            elif change == "empty":
                next_release["assets"][0]["size"] = 0
            elif change == "foreign-url":
                next_release["assets"][0]["browser_download_url"] = "https://untrusted.example.invalid/program.exe"
            elif change == "invalid-date":
                next_release["published_at"] = "invalid"
            else:
                next_release["tag_name"] = "unsafe) markdown"
            with self.subTest(change=change):
                self.assertEqual(links.newest_complete([next_release, current]), current)

    def test_refresh_preserves_other_readme_content_and_is_idempotent(self):
        before = "# Project\n\n" + links.START + "old content" + links.END + "\nUnrelated documentation\n"
        for german in (False, True):
            replacement = links.block(release(), german)
            after = links.replace_block(before, replacement)
            self.assertEqual(links.replace_block(after, replacement), after)
            self.assertTrue(after.startswith("# Project\n\n"))
            self.assertTrue(after.endswith("\nUnrelated documentation\n"))
            self.assertIn("/archive/refs/heads/main.zip", after)
            self.assertIn("/releases/download/v1.2610.29/", after)
            self.assertIn("/compare/v1.2610.29...main", after)
            self.assertIn(r"D:\OpenCode", after)
            self.assertNotIn(r"D:\\OpenCode", after)

    def test_removed_releases_remove_stale_links_but_keep_current_docker_download(self):
        self.assertIsNone(links.newest_complete([]))
        content = links.block(None)
        self.assertIn("/archive/refs/heads/main.zip", content)
        self.assertNotIn(".exe)", content)

    def test_missing_or_duplicate_markers_fail_without_rewriting_prose(self):
        for original in ("plain prose", links.START + links.END + links.START, links.END + links.START):
            with self.assertRaises(ValueError):
                links.replace_block(original, "replacement")


if __name__ == "__main__":
    unittest.main()
