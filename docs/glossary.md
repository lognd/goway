# Glossary

Plain explanations of the words goway's documentation uses.

## Computers and networks

| Word | What it means |
|---|---|
| main laptop | The laptop you type on. Also called the client. |
| helper laptop | Another laptop that does the work. Also called a host. All helpers together are the pool. |
| terminal | A window where you type commands and press Enter instead of clicking. |
| command | One line you type into the terminal, such as `goway status`. |
| PowerShell | The Windows terminal. Open it from the Start menu. |
| administrator | Full rights on Windows. Windows asks "Do you want to allow this app to make changes?" (that prompt is called UAC). |
| WSL | Windows Subsystem for Linux: a Linux system that runs inside Windows. |
| distro, Ubuntu | The kind of Linux inside WSL. Ubuntu is the default. |
| network address, IP | The number a computer has on the network. Wi-Fi often changes it, so goway finds helpers by name instead. |
| `.local` name | A way computers on the same network find each other by name, such as `my-helper.local`. |
| port | A numbered door on a computer that one program listens at. goway's helpers listen at 2222. |
| firewall rule | A permission that lets other computers reach a port. |
| Public / Private network | How Windows classifies a network. Cafes are Public. Your home should be Private. |
| mirrored networking | A WSL setting that lets other computers reach the Linux inside Windows. |
| keepalive task | A Windows task that keeps WSL running so a helper stays reachable. |

## Logging in securely

| Word | What it means |
|---|---|
| ssh | The secure way one computer logs into another to run commands. Everything it sends is encrypted. |
| ssh server, sshd | The program on the helper that accepts those logins. |
| key, key pair | Two matching files that let your laptop prove who it is without a password. The private half never leaves your laptop; the public half goes to the helper. |
| `authorized_keys` | The file on a helper that lists which keys may log in. |
| host key, fingerprint | The helper's own identity, and a short code (`SHA256:...`) to compare it by. goway checks it so it never talks to the wrong machine. |
| pin | Remember a helper's host key; goway refuses any machine that presents another. |
| `known_hosts` | The file where goway keeps the pinned keys. |
| ssh-agent | A background program that holds your keys unlocked. |
| sudo, root | Running a Linux command with administrator rights. It asks for that computer's Linux password. |
| `--rsudo`, `--lsudo` | goway options that allow administrator changes on a helper (`r` for remote) or on this laptop (`l` for local), after asking you once. |

## Projects and builds

| Word | What it means |
|---|---|
| git | The tool that tracks the files of a software project. |
| repository, project folder | A folder git tracks. goway runs commands from inside one. |
| worktree | One checked-out copy of a repository; you can have several. |
| tracked / untracked / ignored | Files git knows about / new files / files git is told to skip (listed in `.gitignore`). goway sends the first two. |
| env file, secret | A file holding passwords or keys, such as `.env`. goway keeps these on your laptop. |
| Rust, cargo | A programming language and its build tool. |
| rustup, cargo-nextest, sccache | Rust's installer, a fast test runner, and a build cache. |
| build cache, warm | Saved results of earlier builds, so the next build is fast. |
| exit code | The number a command ends with. 0 means success. |
| shard | One part of a test run split across several helpers. |
| load | How busy a computer is. goway picks the helper with the lowest load per processor core. |

## goway's own words

| Word | What it means |
|---|---|
| sync | Copying the changed files of your project to a helper. |
| seed | goway's saved copy of your project on a helper, so only changes are sent next time. |
| work dir | A private folder for one run. It is deleted when the run ends. |
| gc | "Garbage collection": deleting old leftovers. goway does it on its own. |
| journal, record | goway's list of every change it made and what was there before, used to undo them exactly. |
| doctor | `goway doctor`: checks everything and says how to fix it. |
| `--` | In `goway run -- COMMAND`, separates goway's options from the command the helper runs. |
| PATH | The list of folders where the terminal looks for programs. |
| `~` | Your home folder on Linux. |
