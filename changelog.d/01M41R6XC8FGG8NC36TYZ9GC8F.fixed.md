Output from sharded runs and goway's own messages is written one whole line at a time under a single lock, with long lines split and partial last lines terminated.
