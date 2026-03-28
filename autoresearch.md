# Autoresearch: Bug Hunt

## Objective
Find and fix bugs in aibrowsr through whitebox analysis. Each iteration: identify a bug, write a test that reproduces it, fix the code, verify the test passes.

## Metrics
- **Primary**: bugs_fixed (count, higher is better)
- **Secondary**: tests_added (count)

## How to Run
`./autoresearch.sh` outputs `METRIC bugs_fixed=number` lines.

## Files in Scope
All src/**/*.rs and tests/*.rs

## Off Limits
None — can change anything.

## Guard
`cargo test -- --test-threads=1`

## Constraints
None.

## Search Space
| Dimension | Type | Range/Values | Dependencies |
|-----------|------|--------------|--------------|
| String slicing | categorical | byte slice / char boundary / chars().take() | UTF-8 content |
| Error handling | categorical | unwrap / expect / ? / match | panic vs graceful |
| CDP response parsing | categorical | strict / lenient / default | Chrome version |
| Integer overflow | categorical | checked / saturating / wrapping | platform (32/64) |
| JS injection | categorical | escape / template / serde_json::to_string | user input |

## Problem Profile
**Bug surface classification**:
- Unsafe string slicing (30%) — `&text[..n]` without UTF-8 boundary check
- Silent error swallowing (20%) — `let _ = ...`, `unwrap_or_default()`
- Unwrap panics (20%) — `unwrap()` on non-guaranteed Some/Ok
- Integer truncation (15%) — `as u8`, `as usize`, `as u32`
- JS injection (10%) — CSS selectors injected into eval strings
- Logic errors (5%) — off-by-one, wrong conditions

## Headroom Table
| Bug Type | Est. Bugs | Confidence | Impact |
|----------|-----------|------------|--------|
| String slicing panics | 6 | High | Crash on non-ASCII |
| Unwrap panics | 3 | High | Crash on edge cases |
| Silent error loss | 3 | Medium | Incorrect behavior |
| Integer overflow/trunc | 3 | Medium | 32-bit compat |
| JS injection | 2 | Medium | Security |
| Logic errors | 3 | Low | Varies |

## What's Been Tried
(updated as experiments accumulate)
