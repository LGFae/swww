swww-daemon(1)

# NAME
swww-daemon

# SYNOPSIS
swww-daemon [-q|--quiet] [-f|--format <xrgb|xbgr|rgb|bgr>] [--no-cache]

# OPTIONS

*-f*,*--format* <xrgb|xbgr|rgb|bgr>
	Force the daemon to use a specific wl_shm format.

	IMPORTANT: make sure this is a value your compositor actually supports!
	'swww-daemon' will automatically select the best format for itself during
	initialization; this is only here for fallback, debug, and workaround
	purposes.

*--no-cache*
	Don't search the cache for the last wallpaper for each output.
	Useful if you always want to select which image 'swww' loads manually using
	'swww img'

*-q*,*--quiet*
	Makes the daemon only log errors.

*-h*, *--help*
	Print help (see a summary with '-h')

*-V*, *--version*
	Print version

# DESCRIPTION

The *swww-daemon* will run continuously, waiting for commands in
_${XDG_RUNTIME_DIR}/swww-${WAYLAND_DISPLAY}.socket_ (or
_/tmp/swww/swww-${WAYLAND_DISPLAY}.socket_, if $XDG_RUNTIME_DIR is not set). The
daemon will take care of both creating and deleting that file when it is
initialized or killed.

# SEE ALSO
*swww-init*(1)
