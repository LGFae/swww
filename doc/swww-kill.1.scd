swww-kill(1)

# NAME
swww-kill

# SYNOPSIS
*swww kill*

# OPTIONS

*-h*, *--help*
	Print help (see a summary with '-h')

# DESCRIPTION

Kills the daemon. This is the recommended way of doing it, since we wait to make
sure the socket file was deleted, thus confirming the daemon exited.

Note that sending SIGTERM to the daemon would work correctly, but sending
SIGKILL would make daemon leave behind the socket file. This is not a big
problem; it would only cause a warning to be printed next time the daemon is
initialized.

# SEE ALSO
*swww-daemon*(1)
