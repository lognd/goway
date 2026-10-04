+++
id = "01M41Y5J0TM325Z5HTA6JC6FR2"
title = "C and C++ builds stay warm and share downloads: compiler launcher via sccache or ccache, shared FetchContent and CPM source caches"
type = "story"
category = "done"
outcome = "done"
priority = "medium"
points = 3
parent = "01M41P2FPBSV4WFSQDGQY7SC89"
reporter = "lognd"
created = "2026-10-03T22:29:16Z"
updated = "2026-10-04T00:14:32Z"
scope = ["crates/goway/src/remote.sh", "docs/usage.md", "docs/config.md", "crates/goway/tests/cxx_launcher.rs"]

[[acceptance]]
text = "Given a CMake project and sccache or ccache on the helper, when goway runs it, then CMAKE_C_COMPILER_LAUNCHER and CMAKE_CXX_COMPILER_LAUNCHER (environment, CMake 3.17+) point at it unless the user set them, using the per-repository cache, so a fresh slot or worktree compiles from cache"
bound = true

[[acceptance]]
text = "Given CPM.cmake projects, when goway runs them, then CPM_SOURCE_CACHE points at a per-repository directory shared by all slots unless the user set it; given plain FetchContent, then docs explain that FetchContent downloads live in each slot's build/_deps (kept warm per slot) and how to share them with FETCHCONTENT_BASE_DIR, which goway never injects into the user's command line"
bound = true

[[acceptance]]
text = "Given goway's coexistence contract, when any of these variables is already set by the user or the project, then goway leaves it untouched"
bound = true
+++
