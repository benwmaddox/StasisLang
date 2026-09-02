import copy
import json
import unittest

from tools.ci import check_host_runtime_contract as contract


class HostRuntimeContractTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.registry = json.loads((contract.ROOT / contract.REGISTRY).read_text(encoding="utf-8"))

    def test_repository_contract_passes(self):
        failures, evidence = contract.check(registry=copy.deepcopy(self.registry))
        self.assertEqual([], failures)
        self.assertEqual("passed", evidence["status"])
        self.assertGreater(evidence["checks"], 300)

    def test_unknown_registry_version_is_rejected(self):
        registry = copy.deepcopy(self.registry)
        registry["version"] = 2
        failures, _ = contract.check(registry=registry)
        self.assertTrue(any(failure.field == "registry.envelope" for failure in failures))

    def test_missing_registry_field_is_rejected(self):
        registry = copy.deepcopy(self.registry)
        del registry["asset_package"]
        failures, _ = contract.check(registry=registry)
        self.assertTrue(any(failure.field == "registry.fields" for failure in failures))

    def test_host_source_drift_names_field_and_source(self):
        source = (contract.ROOT / contract.abi.HOST_FRAME).read_text(encoding="utf-8")
        overlays = {contract.abi.HOST_FRAME: source.replace("const HOST_I_TICK_INDEX: i32 = 10;", "const HOST_I_TICK_INDEX: i32 = 11;", 1)}
        failures, _ = contract.check(registry=copy.deepcopy(self.registry), overlays=overlays)
        failure = next(item for item in failures if item.field == "host_frame.constants.HOST_I_TICK_INDEX")
        self.assertEqual("src/stdlib/internal/host_frame_raw.stasis", failure.source)

    def test_contract_envelope_fixtures_lock_version_behavior(self):
        fixture_root = contract.ROOT / "contracts/v1/fixtures"
        valid = json.loads((fixture_root / "valid_envelope.json").read_text(encoding="utf-8"))
        unsupported = json.loads((fixture_root / "unsupported_version.json").read_text(encoding="utf-8"))
        self.assertIsNone(contract.validate_envelope(valid))
        self.assertEqual("unsupported contract version", contract.validate_envelope(unsupported))

    def test_development_swap_receipt_drift_is_rejected(self):
        source = (contract.ROOT / contract.DEVELOPMENT_SWAP).read_text(encoding="utf-8")
        overlays = {
            contract.DEVELOPMENT_SWAP: source.replace(
                "pub schema_version: u16,", "pub receipt_version: u16,", 1
            )
        }
        failures, _ = contract.check(
            registry=copy.deepcopy(self.registry), overlays=overlays
        )
        self.assertTrue(
            any(
                failure.field == "development_swap.receipt_fields"
                for failure in failures
            )
        )


if __name__ == "__main__":
    unittest.main()
