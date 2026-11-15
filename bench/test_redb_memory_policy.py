"""Comparator policy tests for the A3 memory decision."""

import unittest

import redb_memory_policy as policy

MIB = 1024 * 1024
BASELINE_POST = 478_220_288
BASELINE_PEAK = 488_243_200
A2_COMBINED_POST = 661_000_192
A2_COMBINED_PEAK = 661_348_352


class A3PolicyTests(unittest.TestCase):
    def test_old_policy_rejects_the_a2_combined_result(self):
        self.assertFalse(
            policy.accepted_by_a2_old_policy(
                BASELINE_POST,
                A2_COMBINED_POST,
                BASELINE_PEAK,
                A2_COMBINED_PEAK,
            )
        )

    def test_a3_accepts_the_recorded_combined_result(self):
        self.assertTrue(
            policy.accepted_by_a3(
                BASELINE_POST,
                A2_COMBINED_POST,
                BASELINE_PEAK,
                A2_COMBINED_PEAK,
            )
        )

    def test_a3_rejects_a_result_above_the_new_limit(self):
        limit = policy.post_build_limit(BASELINE_POST)
        self.assertFalse(
            policy.accepted_by_a3(
                BASELINE_POST,
                limit + 1,
                BASELINE_PEAK,
                A2_COMBINED_PEAK,
            )
        )

    def test_a3_peak_limit_is_independent(self):
        limit = policy.peak_limit(BASELINE_PEAK)
        self.assertTrue(limit > policy.post_build_limit(BASELINE_POST))
        self.assertFalse(
            policy.accepted_by_a3(
                BASELINE_POST,
                A2_COMBINED_POST,
                BASELINE_PEAK,
                limit + 1,
            )
        )

    def test_absolute_allowance_controls_small_baselines(self):
        self.assertEqual(policy.post_build_limit(1 * MIB), 33 * MIB)
        self.assertEqual(policy.peak_limit(1 * MIB), 65 * MIB)


if __name__ == "__main__":
    unittest.main()
