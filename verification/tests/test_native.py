import copy
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

DIRECTORY = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("check_native", DIRECTORY / "check_native.py")
native = importlib.util.module_from_spec(spec)
spec.loader.exec_module(native)


class NativeEvidenceTests(unittest.TestCase):
    def test_sources_cannot_silently_retarget_proofs(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            lock = {"version": 1, "policy_sha256": native.digest(b"policy"),
                    "api_sha256": native.digest(b"api"),
                    "targets": {"solana": "solana", "stellar": "stellar"}, "binding": "test"}
            for target in lock["targets"]:
                (root / target).mkdir()
                (root / target / "policy.rs").write_bytes(b"policy")
                (root / target / "policy_api.rs").write_bytes(b"api")
                values = list(bytes.fromhex(lock["policy_sha256"]))
                (root / target / "source_hash.rs").write_text(
                    "pub const SOURCE_HASH: [u8; 32] = " + str(values) + ";\n")
            with patch.object(native, "ROOT", root):
                self.assertEqual(set(native.read_sources(lock)), {"solana", "stellar"})
                (root / "stellar" / "policy_api.rs").write_bytes(b"changed")
                with self.assertRaisesRegex(ValueError, "Stale source binding"):
                    native.read_sources(lock)

    def test_unknown_lock_version_and_missing_target_refuse(self):
        lock = json.loads((DIRECTORY / "native-source-lock.json").read_text())
        for version in (2, True):
            altered = copy.deepcopy(lock)
            altered["version"] = version
            with self.assertRaisesRegex(ValueError, "Unsupported source-lock schema"):
                native.read_sources(altered)
        altered = copy.deepcopy(lock)
        del altered["targets"]["stellar"]
        with self.assertRaisesRegex(ValueError, "Both native source targets"):
            native.read_sources(altered)

    def test_corpus_must_have_complete_matching_results(self):
        vector = (True, 10, 25, 15, 1, 86401)
        self.assertEqual(native.expected(vector), "ok 25")
        self.assertIsNone(native.mismatch([vector], ["ok 25"], ["ok 25"]))
        result = native.mismatch([vector], ["ok 25"], ["err DailyLimitExceeded"])
        self.assertEqual(result["input"]["amount"], 10)
        with self.assertRaisesRegex(ValueError, "incomplete result corpus"):
            native.mismatch([vector], ["ok 25"], [])

    def test_embedded_hash_cannot_claim_different_policy(self):
        source = "pub const SOURCE_HASH: [u8; 32] = " + str([0] * 32) + ";"
        native.embedded_policy_hash(source, "00" * 32)
        with self.assertRaisesRegex(ValueError, "does not match"):
            native.embedded_policy_hash(source, "01" * 32)
        with self.assertRaisesRegex(ValueError, "does not match"):
            native.embedded_policy_hash(source.replace("[0,", "[256,", 1), "00" * 32)

    def test_receipt_binds_current_dependencies_and_source_lock(self):
        receipt = json.loads((DIRECTORY / "native-correspondence.json").read_text())
        lock = json.loads((DIRECTORY / "native-source-lock.json").read_text())
        self.assertEqual(receipt["evidence"], "tested_correspondence")
        self.assertEqual(receipt["policySha256"], lock["policy_sha256"])
        self.assertEqual(receipt["apiSha256"], lock["api_sha256"])
        self.assertEqual(receipt["modelSha256"],
                         native.digest((DIRECTORY / "lean" / "NativeDaily.lean").read_bytes()))
        self.assertEqual(receipt["proofCheckLogSha256"],
                         native.digest((DIRECTORY / "lean" / "proof-check.txt").read_bytes()))
        for path, digest in receipt["verificationDependencies"].items():
            self.assertEqual(native.digest((DIRECTORY / path).read_bytes()), digest)


if __name__ == "__main__":
    unittest.main()
