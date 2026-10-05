Each job now runs in a scope capped at job_tasks processes (default 4096) with a low CPU weight, plus optional job_cpu and job_memory caps per host, and a ulimit -u fallback without systemd.
