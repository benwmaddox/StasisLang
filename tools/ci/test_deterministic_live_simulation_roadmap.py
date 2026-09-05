import copy
import unittest

from tools.ci import check_deterministic_live_simulation_roadmap as roadmap_check


class DeterministicLiveSimulationRoadmapTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.documents = roadmap_check.load_documents()

    def assert_mutation_fails(self, relative: str, old: str, new: str = "") -> None:
        mutated = copy.deepcopy(self.documents)
        self.assertIn(old, mutated[relative])
        mutated[relative] = mutated[relative].replace(old, new, 1)
        with self.assertRaises(roadmap_check.RoadmapError):
            roadmap_check.validate_documents(mutated)

    def test_current_documents_pass(self) -> None:
        roadmap_check.validate_documents(self.documents)

    def test_missing_boundary_fails(self) -> None:
        self.assert_mutation_fails(
            roadmap_check.ROADMAP,
            "The host owns windowing, rendering, audio, filesystem, network",
            "The host owns windowing only",
        )

    def test_native_float_profile_limit_fails(self) -> None:
        self.assert_mutation_fails(
            roadmap_check.ROADMAP,
            "Native float disqualifies cross-architecture strict claims",
            "Native float permits cross-architecture strict claims",
        )

    def test_replay_float_profile_limit_fails(self) -> None:
        self.assert_mutation_fails(
            roadmap_check.ROADMAP,
            "Replay may include native float only under a recorded same-target/toolchain\nprofile",
            "Replay may include native float under any target profile",
        )

    def test_local_profile_strength_fails(self) -> None:
        self.assert_mutation_fails(
            roadmap_check.ROADMAP,
            "Local is the weakest profile",
            "Local is the strongest profile",
        )

    def test_gate_reordering_fails(self) -> None:
        mutated = copy.deepcopy(self.documents)
        roadmap = mutated[roadmap_check.ROADMAP]
        first = "| G2 | #155 |"
        second = "| G3 | #147 |"
        self.assertIn(first, roadmap)
        self.assertIn(second, roadmap)
        roadmap = roadmap.replace(first, "| G2 | #147 |", 1)
        roadmap = roadmap.replace(second, "| G3 | #155 |", 1)
        mutated[roadmap_check.ROADMAP] = roadmap
        with self.assertRaises(roadmap_check.RoadmapError):
            roadmap_check.validate_documents(mutated)

    def test_missing_backlink_fails(self) -> None:
        self.assert_mutation_fails(
            "docs/spec.md",
            "(deterministic_live_simulation_roadmap.md)",
        )

    def test_broken_roadmap_link_fails(self) -> None:
        self.assert_mutation_fails(
            roadmap_check.ROADMAP,
            "(live-compilation-prd.md)",
            "(missing-live-compilation-prd.md)",
        )

    def test_dependency_on_later_gate_fails(self) -> None:
        self.assert_mutation_fails(
            roadmap_check.ROADMAP,
            "| G1 | #146 | Tick safe point and deterministic commit foundation | -- |",
            "| G1 | #146 | Tick safe point and deterministic commit foundation | #155 |",
        )


if __name__ == "__main__":
    unittest.main()
