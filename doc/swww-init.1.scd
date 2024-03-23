swww-init(1)

# NAME
swww-init (DEPRECATED)

# SYNOPSIS
*swww init* [--no-daemon] [--no-cache] [--format <xrgb|xbgr|rgb|bgr>]

# OPTIONS

*--no-daemon*
	Don't fork the daemon. This will keep it running in the current terminal.

	The only reason to do this would be to see the daemon's logs. Note that for
	release builds we only log info, warnings and errors, so you won't be seeing
	much (ideally). This is mostly useful for debugging and developing.

*--no-cache*
	Don't load the cache *during initialization* (it still loads on monitor
	(re)connection).

	If want to always pass an image for 'swww' to load, this option can help make the
	results some reliable: 'swww init --no-cache && swww img <some img>'

*--format* <xrgb|xbgr|rgb|bgr>
	Force the daemon to use a specific wl_shm format.

	IMPORTANT: make sure this is a value your compositor actually supports!
	'swww-daemon' will automatically select the best format for itself during
	initialization; this is only here for fallback, debug, and workaround
	purposes.

*-h*, *--help*
	Print help (see a summary with '-h')

# DESCRIPTION

Initializes the daemon. This is used to be the recommended way of doing it, but
that is no longer the case. You should call 'swww-daemon' directly instead.

# SEE ALSO
*swww-daemon*(1)
