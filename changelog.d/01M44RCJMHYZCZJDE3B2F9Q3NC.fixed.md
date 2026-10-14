goway's own tests no longer leave real systemd scopes behind on a helper when they run inside a goway run: test worlds use a fake systemd-run unless a test opts into a real scope.
