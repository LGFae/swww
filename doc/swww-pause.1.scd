swww-pause(1)

# NAME
swww-pause

# SYNOPSIS
*swww pause*

# OPTIONS

*-a*,*--all*
	Send this command to all active `swww-daemon` namespaces.

*-n*,*--namespace* <namespace>
	Which wayland namespace to send this command to.

	You can specify this option multiple times to send commands to multiple
	namespaces at once: `swww pause -n <namespace1> -n <namespace 2>`.

	This affects the name of the `swww-daemon` socket we will use to
	communicate with. Specifically, our socket name is
	_${WAYLAND_DISPLAY}-swww-daemon.<specified namespace>.socket_.

	Some compositors can have several different wallpapers per output. This
	allows you to differentiate between them. Most users will probably not have
	to set anything in this option.

*-h*, *--help*
	Print help (see a summary with '-h')

# DESCRIPTION

Toggles the pause state of the daemon. A animating gif will become frozen on the
last rendered frame until the command is run again.

Note that this will only pause the animation rendering of the daemon. It will
still respond to swww client commands.

# SEE ALSO
*swww-daemon*(1)
