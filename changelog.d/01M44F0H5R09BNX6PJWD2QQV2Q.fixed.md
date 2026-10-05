A run's wait for a free build slot on a helper is now bounded by --wait, says how many slots are busy and how long the oldest holder has run, and exits 125 on timeout (Unix and Windows helpers).
