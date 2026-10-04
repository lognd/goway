+++
id = "01M424MWHF90WA5AGEX5JN7ZPF"
title = "doctor reads CMake's own interfaces: File API replies and a json-v1 configure trace, with system libraries mapped to packages"
type = "story"
category = "done"
outcome = "done"
priority = "medium"
reporter = "lognd"
created = "2026-10-04T00:22:30Z"
updated = "2026-10-04T05:21:26Z"
scope = ["crates/goway/src/cmakeapi.rs", "crates/goway/src/remote.sh", "crates/goway/tests/cmake_api.rs", "crates/goway/tests/fixtures/cxxshard/CMakeLists.txt", "docs/doctor.md", "docs/cmake.md", "crates/goway/src/lib.rs", "crates/goway/src/doctor.rs", "crates/goway/src/cli.rs", "crates/goway/src/doctor/projneeds.rs", "docs/usage.md", "crates/goway/src/doctor/cmakecheck.rs", "crates/goway/src/add.rs", "crates/goway/tests/fixtures/cmake_api/cmakefind/rc", "crates/goway/tests/fixtures/cmake_api/cmakefind/stderr.txt", "crates/goway/tests/fixtures/cmake_api/cmakefind/trace.jsonl", "crates/goway/tests/fixtures/cmake_api/cmakeplain/rc", "crates/goway/tests/fixtures/cmake_api/cmakeplain/reply/cache-v2-9362f73347a767bf3c3a.json", "crates/goway/tests/fixtures/cmake_api/cmakeplain/reply/cmakeFiles-v1-66e1c06e9d4138f64581.json", "crates/goway/tests/fixtures/cmake_api/cmakeplain/reply/codemodel-v2-9a6b06f81fe458eaf7f3.json", "crates/goway/tests/fixtures/cmake_api/cmakeplain/reply/index-2026-10-04T04-19-04-0823.json", "crates/goway/tests/fixtures/cmake_api/cmakeplain/reply/target-plain-e528f46aa4aae0f806ef.json", "crates/goway/tests/fixtures/cmake_api/cmakeplain/reply/toolchains-v1-c53f300132ef10ed09e2.json", "crates/goway/tests/fixtures/cmake_api/cmakeplain/stderr.txt", "crates/goway/tests/fixtures/cmake_api/cmakeplain/trace.jsonl", "crates/goway/tests/fixtures/cmake_api/cxxshard/rc", "crates/goway/tests/fixtures/cmake_api/cxxshard/reply/cache-v2-4cedbd55425458d97779.json", "crates/goway/tests/fixtures/cmake_api/cxxshard/reply/cmakeFiles-v1-29091b3b4730ec8c3ea0.json", "crates/goway/tests/fixtures/cmake_api/cxxshard/reply/codemodel-v2-ddee2dc57527bc334705.json", "crates/goway/tests/fixtures/cmake_api/cxxshard/reply/index-2026-10-04T04-19-04-0114.json", "crates/goway/tests/fixtures/cmake_api/cxxshard/reply/target-Catch2-bc2eae3abdfe6ce053b9.json", "crates/goway/tests/fixtures/cmake_api/cxxshard/reply/target-Catch2WithMain-ca33a633470ba8e90cb0.json", "crates/goway/tests/fixtures/cmake_api/cxxshard/reply/target-c2-d21b5a9f893a2c368271.json", "crates/goway/tests/fixtures/cmake_api/cxxshard/reply/target-gmock-353ba69e83eed568afd0.json", "crates/goway/tests/fixtures/cmake_api/cxxshard/reply/target-gmock_main-ea7d66f1e466a7a0a3c2.json", "crates/goway/tests/fixtures/cmake_api/cxxshard/reply/target-gt-b7ed8d20913561c64465.json", "crates/goway/tests/fixtures/cmake_api/cxxshard/reply/target-gtest-0a2ed7901097b356cdf2.json", "crates/goway/tests/fixtures/cmake_api/cxxshard/reply/target-gtest_main-226d36c068ff8ec1d4be.json", "crates/goway/tests/fixtures/cmake_api/cxxshard/reply/toolchains-v1-0a8e85d95c1075b5bb22.json", "crates/goway/tests/fixtures/cmake_api/cxxshard/stderr.txt", "crates/goway/tests/fixtures/cmake_api/cxxshard/trace.jsonl", "crates/goway/tests/fixtures/cmakefind/CMakeLists.txt", "crates/goway/tests/fixtures/cmakefind/gt.cpp", "crates/goway/tests/fixtures/cmakeplain/CMakeLists.txt", "crates/goway/tests/fixtures/cmakeplain/main.cpp"]

[[acceptance]]
text = "Given a CMake project, when doctor determines its needs, then it uses CMake's own interfaces, not text matching: goway places a File API query (codemodel-v2, cache-v2, cmakeFiles-v1, toolchains-v1) in every build directory it keeps in a slot so normal configures write replies, doctor reads the latest replies (compilers with id and version, <Pkg>_DIR and FETCHCONTENT_* cache entries, fetched dependencies, link libraries), and doctor --configure runs one configure in a labelled scratch directory with --trace-expand --trace-format=json-v1 to capture find_package, FetchContent_Declare, pkg_check_modules and CPMAddPackage calls with resolved arguments; a failed find_package names the missing package and its install command"
bound = true

[[acceptance]]
text = "Given the packages a CMake project finds (find_package(GTest) needs libgtest-dev, find_package(Catch2) needs catch2, pkg_check_modules entries), when doctor runs, then one table maps them to apt, dnf and pacman package names, and FetchContent and CPM dependencies need no system package"
bound = true

[[acceptance]]
text = "Given doctor analyses a project, when it configures it, then that only happens on helpers inside goway's labelled root (never on the laptop and never in the user's work tree), every reply is read with size limits and parsed strictly, and the scratch directory is removed afterwards and covered by gc"
bound = true
+++

Split from ~XP4RZVS (criteria 2 library part, 5 and 7), which landed the detection, checks and fixes that this builds on. Test against the real cxxshard project and a find_package(GTest) variant.
