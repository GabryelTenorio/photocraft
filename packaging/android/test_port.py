"""Regression tests for preserving newer upstream catalog entries during a port."""
import unittest
from port import entries, merge_missing


class CatalogPortTests(unittest.TestCase):
    def test_newer_translation_and_unrelated_rows_are_retained(self):
        original = '# Header\n\tSet Sample Point\tNew upstream wording\n@id\ttool.move\tMove\n'
        merged = merge_missing(original, [['', 'Set Sample Point', 'Old fork wording'], ['', 'Touch Tools', 'Tools']])
        self.assertIn(original.strip(), merged)
        self.assertEqual(entries(merged)[('', 'Set Sample Point')], 'New upstream wording')
        self.assertEqual(entries(merged)[('@id', 'tool.move')], 'Move')
        self.assertEqual(merge_missing(merged, [['', 'Touch Tools', 'Tools']]), merged)

    def test_contexts_do_not_collide(self):
        merged = merge_missing('\tOpen\tOpen file\n', [['@dialog', 'Open', 'Open dialog']])
        self.assertEqual(len(entries(merged)), 2)

    def test_malformed_or_duplicate_catalogs_fail(self):
        for text in ['broken row', '\tOpen\tOne\n\tOpen\tTwo\n']:
            with self.assertRaises(ValueError):
                entries(text)


if __name__ == '__main__':
    unittest.main()
