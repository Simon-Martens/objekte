"""A3 memory policy for the redb primary-index feasibility campaign.

The original 1.15/1.20 factors are retained as historical results. A3 adopts
1.40/1.60 for resident and peak memory respectively, with absolute allowances
of 32 MiB and 64 MiB. This module is intentionally small so the comparator
tests can reject a candidate above the recorded policy.
"""

MIB = 1024 * 1024

A3_POST_BUILD_FACTOR = 1.40
A3_POST_QUERY_FACTOR = 1.40
A3_POST_BUILD_ALLOWANCE = 32 * MIB
A3_PEAK_FACTOR = 1.60
A3_PEAK_ALLOWANCE = 64 * MIB

# Historical values, used only to show that the old failure is not relabelled.
A2_OLD_POST_BUILD_FACTOR = 1.15
A2_OLD_POST_BUILD_ALLOWANCE = 32 * MIB
A2_OLD_PEAK_FACTOR = 1.20
A2_OLD_PEAK_ALLOWANCE = 64 * MIB


def resident_limit(native_bytes: int, factor: float, allowance: int) -> int:
    return max(int(native_bytes * factor), native_bytes + allowance)


def post_build_limit(native_bytes: int, factor: float = A3_POST_BUILD_FACTOR) -> int:
    return resident_limit(native_bytes, factor, A3_POST_BUILD_ALLOWANCE)


def peak_limit(native_bytes: int, factor: float = A3_PEAK_FACTOR) -> int:
    return resident_limit(native_bytes, factor, A3_PEAK_ALLOWANCE)


def accepted_by_a3(
    native_post_build_bytes: int,
    candidate_post_build_bytes: int,
    native_peak_bytes: int,
    candidate_peak_bytes: int,
) -> bool:
    return (
        candidate_post_build_bytes <= post_build_limit(native_post_build_bytes)
        and candidate_peak_bytes <= peak_limit(native_peak_bytes)
    )


def accepted_by_a2_old_policy(
    native_post_build_bytes: int,
    candidate_post_build_bytes: int,
    native_peak_bytes: int,
    candidate_peak_bytes: int,
) -> bool:
    return (
        candidate_post_build_bytes
        <= resident_limit(
            native_post_build_bytes,
            A2_OLD_POST_BUILD_FACTOR,
            A2_OLD_POST_BUILD_ALLOWANCE,
        )
        and candidate_peak_bytes
        <= resident_limit(
            native_peak_bytes,
            A2_OLD_PEAK_FACTOR,
            A2_OLD_PEAK_ALLOWANCE,
        )
    )
