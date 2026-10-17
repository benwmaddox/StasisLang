import unittest

from tools import generate_sdl_scancodes as generator


def header(body: str) -> str:
    return f"typedef enum SDL_Scancode {{\n{body}\n}} SDL_Scancode;"


class SdlScancodeGeneratorTests(unittest.TestCase):
    def test_explicit_header_entries_and_top_row_digit_mapping(self):
        entries = generator.parse_scancode_header(
            header(
                """
                SDL_SCANCODE_UNKNOWN = 0,
                SDL_SCANCODE_A = 4,
                SDL_SCANCODE_1 = 30,
                SDL_SCANCODE_0 = 39,
                SDL_SCANCODE_LANG6 = 149,
                SDL_SCANCODE_LANG9 = 152,
                SDL_SCANCODE_RESERVED = 400,
                SDL_SCANCODE_COUNT = 512
                """
            )
        )
        self.assertEqual(
            entries,
            [
                generator.Scancode("A", 4),
                generator.Scancode("DIGIT_1", 30),
                generator.Scancode("DIGIT_0", 39),
                generator.Scancode("LANG6", 149, True),
                generator.Scancode("LANG9", 152, True),
            ],
        )

    def test_markers_are_required_and_must_keep_their_contract_values(self):
        missing_count = header(
            "SDL_SCANCODE_UNKNOWN = 0, SDL_SCANCODE_A = 4, SDL_SCANCODE_RESERVED = 400"
        )
        with self.assertRaisesRegex(ValueError, "marker set changed"):
            generator.parse_scancode_header(missing_count)

        changed_count = header(
            "SDL_SCANCODE_UNKNOWN = 0, SDL_SCANCODE_A = 4, "
            "SDL_SCANCODE_RESERVED = 400, SDL_SCANCODE_COUNT = 513"
        )
        with self.assertRaisesRegex(ValueError, "changed from 512 to 513"):
            generator.parse_scancode_header(changed_count)

    def test_numeric_aliases_fail_closed(self):
        source = header(
            "SDL_SCANCODE_UNKNOWN = 0, SDL_SCANCODE_A = 4, SDL_SCANCODE_B = 4, "
            "SDL_SCANCODE_RESERVED = 400, SDL_SCANCODE_COUNT = 512"
        )
        with self.assertRaisesRegex(ValueError, "aliases need an explicit policy"):
            generator.parse_scancode_header(source)

    def test_digit_mapping_name_collisions_fail_closed(self):
        source = header(
            "SDL_SCANCODE_UNKNOWN = 0, SDL_SCANCODE_1 = 30, "
            "SDL_SCANCODE_DIGIT_1 = 31, SDL_SCANCODE_RESERVED = 400, "
            "SDL_SCANCODE_COUNT = 512"
        )
        with self.assertRaisesRegex(ValueError, "duplicate emitted SDL scancode name: DIGIT_1"):
            generator.parse_scancode_header(source)

    def test_ordinary_scancodes_cannot_alias_marker_values(self):
        for name, value in (("A", 0), ("B", 400)):
            with self.subTest(name=name, value=value):
                source = header(
                    "SDL_SCANCODE_UNKNOWN = 0, "
                    f"SDL_SCANCODE_{name} = {value}, "
                    "SDL_SCANCODE_RESERVED = 400, SDL_SCANCODE_COUNT = 512"
                )
                with self.assertRaisesRegex(ValueError, "aliases marker (UNKNOWN|RESERVED)"):
                    generator.parse_scancode_header(source)

    def test_duplicate_names_fail_closed(self):
        source = header(
            "SDL_SCANCODE_UNKNOWN = 0, SDL_SCANCODE_A = 4, SDL_SCANCODE_A = 5, "
            "SDL_SCANCODE_RESERVED = 400, SDL_SCANCODE_COUNT = 512"
        )
        with self.assertRaisesRegex(ValueError, "duplicate SDL scancode name: A"):
            generator.parse_scancode_header(source)

    def test_unknown_expressions_and_conditionals_fail_closed(self):
        expression = header(
            "SDL_SCANCODE_UNKNOWN = 0, SDL_SCANCODE_A = SDL_SCANCODE_UNKNOWN + 4, "
            "SDL_SCANCODE_RESERVED = 400, SDL_SCANCODE_COUNT = 512"
        )
        with self.assertRaisesRegex(ValueError, "unrecognized SDL_Scancode declaration"):
            generator.parse_scancode_header(expression)

        conditional = header(
            "SDL_SCANCODE_UNKNOWN = 0,\n#if SDL_FEATURE\nSDL_SCANCODE_A = 4,\n#endif\n"
            "SDL_SCANCODE_RESERVED = 400, SDL_SCANCODE_COUNT = 512"
        )
        with self.assertRaisesRegex(ValueError, "preprocessor conditionals"):
            generator.parse_scancode_header(conditional)

    def test_invalid_top_row_digit_suffix_and_out_of_range_slot_fail(self):
        invalid_digit = header(
            "SDL_SCANCODE_UNKNOWN = 0, SDL_SCANCODE_10 = 30, "
            "SDL_SCANCODE_RESERVED = 400, SDL_SCANCODE_COUNT = 512"
        )
        with self.assertRaisesRegex(ValueError, "unsupported numeric SDL scancode suffix"):
            generator.parse_scancode_header(invalid_digit)

        out_of_range = header(
            "SDL_SCANCODE_UNKNOWN = 0, SDL_SCANCODE_A = 512, "
            "SDL_SCANCODE_RESERVED = 400, SDL_SCANCODE_COUNT = 512"
        )
        with self.assertRaisesRegex(ValueError, "outside HostFrame.keys"):
            generator.parse_scancode_header(out_of_range)

    def test_generated_group_is_deterministic_and_marks_reserved_names(self):
        entries = [
            generator.Scancode("A", 4),
            generator.Scancode("LANG6", 149, True),
        ]
        expected = "\n".join(
            [
                generator.BEGIN_MARKER,
                "const SCANCODE {",
                "    A: i32 = 4;",
                "    // SDL documents LANG6-LANG9 as reserved named slots.",
                "    LANG6: i32 = 149;",
                "}",
                "",
                generator.END_MARKER,
            ]
        )
        rendered = generator.render_group(entries)
        self.assertEqual(rendered, expected)
        self.assertEqual(generator.render_group(entries), rendered)

    def test_group_replacement_preserves_surrounding_legacy_source(self):
        source = (
            "enum Scancode { W = 26, }\n"
            f"{generator.BEGIN_MARKER}\nold generated block\n"
            f"{generator.END_MARKER}\ntrailing comment\n"
        )
        rendered = generator.render_group([generator.Scancode("W", 26)])
        updated = generator.replace_generated_group(source, rendered)
        self.assertTrue(updated.startswith("enum Scancode { W = 26, }\n"))
        self.assertTrue(updated.endswith("\ntrailing comment\n"))
        self.assertIn("    W: i32 = 26;", updated)
        self.assertNotIn("old generated block", updated)

    def test_group_replacement_requires_one_ordered_marker_pair(self):
        with self.assertRaisesRegex(ValueError, "one generated-group marker pair"):
            generator.replace_generated_group("no marker", "generated")
        source = (
            f"{generator.END_MARKER}\n{generator.BEGIN_MARKER}\n"
        )
        with self.assertRaisesRegex(ValueError, "out of order"):
            generator.replace_generated_group(source, "generated")


if __name__ == "__main__":
    unittest.main()
