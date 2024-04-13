swww(1)

# NAME
swww - A Solution to your Wayland Wallpaper Woes

# SYNOPSIS
*swww* <COMMAND>

# COMMANDS

*clear*
	Fills the specified outputs with the given color

*restore*
	Restores the last displayed image on the specified outputs

*clear-cache*
	Fills the specified outputs with the given color

*img*
	Sends an image (or animated gif) for the daemon to display

*init*
	Initializes the daemon

*kill*
	Kills the daemon

*query*
	Asks the daemon to print output information (names and dimensions)

*help [COMMAND]*
	Print help or the help of the given command

# OPTIONS

*-h*, *--help*
	Print help (see a summary with '-h')

*-V*, *--version*
	Print  version

# DESCRIPTION
*swww* is a wallpaper manager that lets you change what your monitors display as
a background by controlling the *swww-daemon* at runtime.

It supports animated gifs and putting different stuff in different monitors. I
also did my best to make it as resource efficient as possible.

To start, begin by running *swww init*. That will set up the *swww-daemon*.
Then, you can send images to be displayed with *swww img*. To kill the daemon,
use *swww kill*.

*Note that swww only works in a compositor that implements the layer-shell
protocol*. Typically, _wlr-roots_ based compositors.

# FILES
*swww* will create the following files in your system:
	- A socket in _${XDG_RUNTIME_DIR}/swww-${WAYLAND_DISPLAY}.socket_,  or
	_/tmp/swww/swww-${WAYLAND_DISPLAY}.socket_, if $XDG_RUNTIME_DIR is not set.
	- Cache files in _$XDG_CACHE_HOME/swww_ or _$HOME/.cache/swww_ if
	  $XDG_CACHE_HOME does not exist. These are used to set the wallpaper to the
	  previous image when a monitor is (re)connected or turned on.

# SEE ALSO
*swww-daemon*(1) *swww-clear*(1) *swww-img*(1) *swww-init*(1) *swww-kill*(1)
*swww-query*(1)
