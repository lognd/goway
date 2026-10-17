The Windows facts probe in doctor now runs rustup and vswhere under a 10 second limit enforced on the host, so a hung program no longer stalls the check; a killed probe is reported as a warning.
