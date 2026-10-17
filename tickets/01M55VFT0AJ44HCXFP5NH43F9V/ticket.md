+++
id = "01M55VFT0AJ44HCXFP5NH43F9V"
title = "Docs: circles.md note on dropped revoked leads in saved listed policy"
type = "task"
category = "in-progress"
priority = "medium"
points = 1
reporter = "lognd"
created = "2026-10-17T21:15:06Z"
updated = "2026-10-17T21:17:57Z"
scope = ["docs/circles.md"]
+++

Follow-up to ~P023524: in the policy bullet (section 4.3) note that a lead in the saved listed policy that is later revoked is dropped with a warning by goway circle access, which still writes new-epoch lines; only explicit --lead names error. Deferred because ~3WFASD5 holds a lease on docs/circles.md.
