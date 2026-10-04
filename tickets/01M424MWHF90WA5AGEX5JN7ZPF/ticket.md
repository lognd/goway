+++
id = "01M424MWHF90WA5AGEX5JN7ZPF"
title = "doctor reads CMake's own interfaces: File API replies and a json-v1 configure trace, with system libraries mapped to packages"
type = "story"
category = "in-progress"
priority = "medium"
reporter = "lognd"
created = "2026-10-04T00:22:30Z"
updated = "2026-10-04T05:20:05Z"
scope = ["crates/goway/src/cmakeapi.rs", "crates/goway/src/remote.sh", "crates/goway/tests/cmake_api.rs", "crates/goway/tests/fixtures/cxxshard/CMakeLists.txt", "docs/doctor.md", "docs/cmake.md", "crates/goway/src/lib.rs", "crates/goway/src/doctor.rs", "crates/goway/src/cli.rs", "crates/goway/src/doctor/projneeds.rs", "docs/usage.md", "crates/goway/src/doctor/cmakecheck.rs", "crates/goway/src/add.rs"]

[[acceptance]]
text = "Given a CMake project, when doctor determines its needs, then it uses CMake's own interfaces, not text matching: goway places a File API query (codemodel-v2, cache-v2, cmakeFiles-v1, toolchains-v1) in every build directory it keeps in a slot so normal configures write replies, doctor reads the latest replies (compilers with id and version, <Pkg>_DIR and FETCHCONTENT_* cache entries, fetched dependencies, link libraries), and doctor --configure runs one configure in a labelled scratch directory with --trace-expand --trace-format=json-v1 to capture find_package, FetchContent_Declare, pkg_check_modules and CPMAddPackage calls with resolved arguments; a failed find_package names the missing package and its install command"
bound = true

[[acceptance]]
text = "Given the packages a CMake project finds (find_package(GTest) needs libgtest-dev, find_package(Catch2) needs catch2, pkg_check_modules entries), when doctor runs, then one table maps them to apt, dnf and pacman package names, and FetchContent and CPM dependencies need no system package"
bound = true

[[acceptance]]
text = "Given doctor analyses a project, when it configures it, then that only happens on helpers inside goway's labelled root (never on the laptop and never in the user's work tree), every reply is read with size limits and parsed strictly, and the scratch directory is removed afterwards and covered by gc"
bound = false
+++

Split from ~XP4RZVS (criteria 2 library part, 5 and 7), which landed the detection, checks and fixes that this builds on. Test against the real cxxshard project and a find_package(GTest) variant.
