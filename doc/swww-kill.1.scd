swww-kill(1)

# NAME
swww-kill

# SYNOPSIS
*swww kill*

# OPTIONS

*-a*,*--all*
	Send this command to all active `swww-daemon` namespaces.

*-n*,*--namespace* <namespace>
	Which wayland namespace to send this command to.

	You can specify this option multiple times to send commands to multiple
	namespaces at once: `swww kill -n <namespace1> -n <namespace 2>`.

	This affects the name of the `swww-daemon` socket we will use to
	communicate with. Specifically, our socket name is
	_${WAYLAND_DISPLAY}-swww-daemon.<specified namespace>.socket_.

	Some compositors can have several different wallpapers per output. This
	allows you to differentiate between them. Most users will probably not have
	to set anything in this option.

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
