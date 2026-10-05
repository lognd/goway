The default per-job task cap is now derived from the helper (half of its threads-max, floored at 16384) instead of a fixed 4096 that starved builds and test runs.
