import unittest

from tools import atlas_affinity_preview as preview


class NativeLogicalPathHashTests(unittest.TestCase):
    def test_fnv1a64_matches_standard_vectors(self):
        self.assertEqual(preview.fnv1a64(""), 0xCBF29CE484222325)
        self.assertEqual(preview.fnv1a64("a"), 0xAF63DC4C8601EC8C)

    def test_normalization_matches_native_slash_and_dot_rules(self):
        cases = {
            "assets/hero.png": "assets/hero.png",
            r"assets\sprites\hero.png": "assets/sprites/hero.png",
            "/assets/./sprites/../hero.png": "assets/hero.png",
            "../assets/hero.png": "assets/hero.png",
        }
        for source, expected in cases.items():
            with self.subTest(source=source):
                self.assertEqual(preview.normalize_asset_path(source), expected)

    def test_virtual_asset_root_cannot_escape_assets(self):
        for source in ("/tmp/hero.png", "/assets/../../tmp/hero.png", r"C:\assets\hero.png", r"\server\hero.png"):
            with self.subTest(source=source):
                with self.assertRaises(ValueError):
                    preview.normalize_asset_path(source)

    def test_hash_is_case_sensitive_like_native_path_identity(self):
        self.assertNotEqual(preview.path_hash("assets/Hero.png"), preview.path_hash("assets/hero.png"))

    def test_native_absolute_logical_path_hash_matches_c_fast_path(self):
        logical_path = "/assets/generated/sheep.png"
        self.assertEqual(preview.native_sprite_path_hash(logical_path), preview.fnv1a64(logical_path))
        self.assertNotEqual(preview.native_sprite_path_hash(logical_path), preview.path_hash(logical_path))
        self.assertEqual(preview.native_sprite_path_hash("assets/./sheep.png"), preview.path_hash("assets/sheep.png"))

    def test_planner_output_parser_ignores_signed_launcher_status_lines(self):
        stdout = (
            "Done Adding Additional Store\n"
            "Successfully signed: target\\debug\\examples\\atlas_affinity_preview.exe\n"
            "True\n"
            '{"schema":"atlas-affinity-plan/v1","accepted":false}\n'
        )
        self.assertEqual(
            preview.parse_production_planner_output(stdout),
            {"schema": "atlas-affinity-plan/v1", "accepted": False},
        )


class AotMappingTests(unittest.TestCase):
    def test_missing_native_endpoint_is_counted_as_unmapped(self):
        import json
        import tempfile
        from pathlib import Path

        manifest = {
            "hot_render_metadata_version": 4,
            "hot_render_images": [
                {"identity": "hero", "logical_path": "/assets/hero.png", "logical_width": 16, "logical_height": 16, "atlas_eligible": True},
                {"identity": "sheep", "logical_path": "/assets/sheep.png", "logical_width": 16, "logical_height": 16, "atlas_eligible": True},
            ],
            "hot_render_transitions": [
                {"from_identity": "hero", "to_identity": "sheep", "max_transitions_per_render": 7, "validity": "finite"}
            ],
            "hot_render_transition_analysis": {
                "validity": "complete", "unknown_causes": [], "pair_count": 1,
                "published_pair_count": 1, "omitted_pair_count": 0, "pair_limit": 4096,
                "max_pair_weight": 1000000000,
            },
        }
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            manifest_path = root / "manifest.json"
            manifest_path.write_text(json.dumps(manifest), encoding="utf-8")
            snapshot = {
                "source": {"aot_manifest_path": str(manifest_path)},
                "sprites": [{"id": 11, "aot_identity": "hero", "identity": "hero"}],
            }
            with self.assertRaisesRegex(ValueError, "found 1 unmapped rows weighing 7"):
                preview.load_aot_pair_weights(snapshot, root / "snapshot.json", None)


class NativePairFlagsTests(unittest.TestCase):
    def test_runtime_pairs_require_valid_flag_and_no_overflow_flags(self):
        snapshot = {
            "source": {"native_flags": 1 | 8},
            "sprites": [{"id": 1}, {"id": 2}],
            "raw_pairs": [{"from_handle": 1, "to_handle": 2, "weight": 3}],
        }
        rows, evidence = preview.runtime_pair_weights(snapshot)
        self.assertEqual(rows, [{"from_sprite_id": 1, "to_sprite_id": 2, "weight": 3}])
        self.assertEqual(evidence["mode"], "bounded-runtime-histogram")

        invalid_flags = (
            (0, "PAIR_VALID"),
            (1 | 2, "PAIR_OVERFLOW"),
            (1 | 4, "INVENTORY_OVERFLOW"),
        )
        for flags, reason in invalid_flags:
            with self.subTest(flags=flags):
                snapshot["source"]["native_flags"] = flags
                with self.assertRaisesRegex(ValueError, reason):
                    preview.runtime_pair_weights(snapshot)


class AotSummaryContractTests(unittest.TestCase):
    def test_aot_summary_requires_exact_production_caps_and_complete_counts(self):
        import copy
        import json
        import tempfile
        from pathlib import Path

        canonical = {
            "hot_render_metadata_version": 4,
            "hot_render_images": [],
            "hot_render_transitions": [],
            "hot_render_transition_analysis": {
                "validity": "complete",
                "unknown_causes": [],
                "pair_count": 0,
                "published_pair_count": 0,
                "omitted_pair_count": 0,
                "pair_limit": 4096,
                "max_pair_weight": 1_000_000_000,
            },
        }
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            manifest_path = root / "manifest.json"
            snapshot = {"source": {"aot_manifest_path": str(manifest_path)}, "sprites": []}

            for field, value, reason in (
                ("pair_limit", 2048, "pair_limit must equal the production cap 4096"),
                ("max_pair_weight", 999_999_999, "max_pair_weight must equal the production cap 1000000000"),
                ("omitted_pair_count", 1, "zero omitted transition rows"),
                ("unknown_causes", ["dynamic loop"], "unknown causes"),
            ):
                with self.subTest(field=field):
                    manifest = copy.deepcopy(canonical)
                    manifest["hot_render_transition_analysis"][field] = value
                    manifest_path.write_text(json.dumps(manifest), encoding="utf-8")
                    with self.assertRaisesRegex(ValueError, reason):
                        preview.load_aot_pair_weights(snapshot, root / "snapshot.json", None)

            incomplete = copy.deepcopy(canonical)
            incomplete["hot_render_transition_analysis"].update(
                validity="incomplete",
                unknown_causes=["unbounded or dynamic for loop"],
                pair_count=None,
                omitted_pair_count=None,
            )
            manifest_path.write_text(json.dumps(incomplete), encoding="utf-8")
            with self.assertRaisesRegex(ValueError, "unbounded or dynamic for loop"):
                preview.load_aot_pair_weights(snapshot, root / "snapshot.json", None)

            manifest_path.write_text(json.dumps(canonical), encoding="utf-8")
            rows, evidence = preview.load_aot_pair_weights(snapshot, root / "snapshot.json", None)
            self.assertEqual(rows, [])
            self.assertEqual(evidence["mode"], "accepted-aot-v4")


class AotImageGeometryTests(unittest.TestCase):
    def test_ineligible_zero_geometry_rows_are_valid_but_eligible_rows_are_rejected(self):
        import json
        import tempfile
        from pathlib import Path

        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            path = root / "manifest.json"
            base = {
                "hot_render_metadata_version": 4,
                "hot_render_images": [
                    {
                        "identity": "dynamic-image",
                        "logical_path": "/assets/dynamic.svg",
                        "logical_width": 0,
                        "logical_height": 0,
                        "atlas_eligible": False,
                    }
                ],
                "hot_render_transitions": [],
            }
            path.write_text(json.dumps(base), encoding="utf-8")
            manifest, _ = preview.read_aot_for_mapping(path)
            self.assertEqual(manifest["hot_render_images"][0]["logical_width"], 0)

            base["hot_render_images"][0]["atlas_eligible"] = True
            path.write_text(json.dumps(base), encoding="utf-8")
            with self.assertRaisesRegex(ValueError, "eligible AOT image.*positive logical dimensions"):
                preview.read_aot_for_mapping(path)


class NativePageExtentTests(unittest.TestCase):
    def test_variable_eligible_extent_and_frozen_dedicated_page_are_preserved(self):
        import tempfile
        from pathlib import Path

        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            raw = {
                "schema": "atlas-affinity-native-query/v1",
                "snapshot_token": 3,
                "renderer_generation": 8,
                "asset_generation": 12,
                "flags": 0,
                "stage_peak_cap_bytes": 100_000_000,
                "evidence_provenance": "bounded-runtime-histogram",
                "pages": [
                    {"page_index": 0, "width": 1024, "height": 1024, "usable_x": 1, "usable_y": 6, "padding": 1, "reserved_header_height": 6, "flags": 1 << 4, "compatibility_flags": 1, "group_id": 7, "allocation_bytes": 4_194_304},
                    {"page_index": 1, "width": 2048, "height": 1024, "usable_x": 1, "usable_y": 6, "padding": 1, "reserved_header_height": 6, "flags": (1 << 4) | 1, "compatibility_flags": 1, "group_id": 7, "allocation_bytes": 8_388_608},
                ],
                "sprites": [],
                "pairs": [],
            }
            snapshot = preview.normalize_native_snapshot(raw, root / "snapshot.json", "SheepHerder", root, None, None)
            self.assertEqual([(page["width"], page["height"]) for page in snapshot["pages"]], [(1024, 1024)])
            self.assertEqual(len(snapshot["native_pages"]), 2)
            self.assertEqual(snapshot["native_pages"][1]["planner_exclusion_reason"], "protected_or_dedicated_page_flags")
            self.assertEqual(snapshot["budget"]["current_device_bytes"], 12_582_912)
            tsv = preview.planner_input_tsv(snapshot, [])
            self.assertIn("page\t0\t1024\t1024\t", tsv)
            outputs = preview.render_layout("extent-test", [], snapshot["native_pages"], {}, root / "out")
            dimensions = []
            for row in outputs:
                with preview.Image.open(root / "out" / row["path"]) as image:
                    dimensions.append(image.size)
            self.assertEqual(dimensions, [(1024, 1024), (2048, 1024)])


if __name__ == "__main__":
    unittest.main()
