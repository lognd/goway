+++
id = "01M544DC8P7E0DTBA25QMYNF4Q"
title = "host add --port 22 refuses a Windows OpenSSH helper; setup advice is Linux-only"
type = "bug"
category = "in-progress"
priority = "high"
points = 5
reporter = "lognd"
created = "2026-10-17T05:12:34Z"
updated = "2026-10-17T05:55:01Z"
idempotency_key = "nova-windows-host-add"
labels = ["windows", "onboarding"]

[[acceptance]]
text = "A Windows sshd that refuses the key is detected by banner and recorded as os=windows when vouched for"
bound = true

[[acceptance]]
text = "host add --windows prints Windows setup commands and writes nothing"
bound = true

[[acceptance]]
text = "the Windows advice names OpenSSH.Server, administrators_authorized_keys, icacls and DefaultShell, not passwd/faillock"
bound = false
+++

Report from another session: 'goway host add nova --address 192.168.1.224 --port 22' fails with 'no Linux sshd answered ... key authentication refused' and suggests 'goway add' (passwd -S, faillock, fail2ban). docs/hosts.md says a Windows sshd is accepted as os=windows when no Linux machine answers, so this may be a bug when the key is not yet authorized. Asks: detect Windows sshd (banner or uname failure) and print Windows fixes (install OpenSSH.Server, authorize key in C:\ProgramData\ssh\administrators_authorized_keys with icacls, set DefaultShell to PowerShell); a print-commands mode (--windows / --no-password); let host add record a Windows ssh-only helper so it appears in host list. Overlaps circles phase 3 (Windows administrators_authorized_keys). Reproduce first.
