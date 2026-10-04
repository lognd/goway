# CMake projects: asking CMake

goway does not read `CMakeLists.txt` as text to learn what a project needs:
CMake can say it itself. Three of its own interfaces are used, all on the
helper, all parsed strictly (a reply that is not the documented JSON is an
error that doctor shows, never a guess).

## The File API replies

Every `goway run` of a CMake project leaves a stateful File API query
(`.cmake/api/v1/query/client-goway/query.json`, asking for `codemodel-v2`,
`cache-v2`, `cmakeFiles-v1` and `toolchains-v1`) in the build directories of the
slot tree it runs in: `build/` (made when absent) and every `cmake-build-*`.
A directory that holds the project's own files and no CMake cache is not a
build directory and is left alone. Any configure there then writes replies.

`goway doctor` reads the newest replies of the repository's slot trees
(`cmake-replies`) and reports:

- the compilers CMake found (language, id, version, path);
- `<Pkg>_DIR` cache entries: a package `find_package` looked for, found or
  `-NOTFOUND`;
- `FETCHCONTENT_SOURCE_DIR_*` entries: dependencies CMake downloads itself,
  which need no system package;
- the libraries targets link by system path or `-l` name (mapped to packages);
- programs a find command lacked (`pkg-config`).

## `goway doctor --configure`

Runs one configure of a snapshot of your work tree on each helper, with
`--trace-expand --trace-format=json-v1`, in the snapshot's own work directory
(labelled with the repository and worktree like every work directory, locked
while it runs, removed when it ends, and collected by `goway gc` if the helper
died meanwhile). It never runs on this machine or in your work tree. It may
download what the project downloads (FetchContent, CPM), into that scratch
directory only, and stops after five minutes.

The trace gives the project's own `find_package`, `FetchContent_Declare`,
`pkg_check_modules` and `CPMAddPackage` calls with their arguments resolved
(calls inside CMake's own modules, fetched dependencies and try-compile
projects are ignored). A configure that fails because `find_package` could not
find something names the package, and doctor prints the install command:

    FAIL  cmake: GTest  missing (find_package(GTest) at CMakeLists.txt:3); install libgtest-dev

`goway doctor --explain "cmake: GTest"` shows the tail of CMake's own message.

## One table of packages

What CMake looks for is mapped to system packages by one table
(`LIB_PACKAGES` in `crates/goway/src/cmakeapi.rs`): the `find_package` name,
the library name and the pkg-config module to the apt, dnf and pacman packages
(`GTest` is `libgtest-dev`, `gtest-devel`, `gtest`). Packages that are CMake's
own (`Threads`, `OpenMP`, ...) and everything `FetchContent` or CPM fetches need
no system package.

## Limits

Every reply and every part of a helper's answer is size-limited: 4 MiB a
file, 400 files and 16 MiB a bundle, 50 000 cache entries, 2 000 targets.
Names are checked against a strict character set before they are used, and an
index that names a file outside its reply directory is refused.
