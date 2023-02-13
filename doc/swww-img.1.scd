swww-img(1)

# NAME
swww-img

# SYNOPSIS
*swww img* [OPTIONS] <path/to/img>

# OPTIONS

*-f*, *--filter* <FILTER>
	Filter to use when scaling images

	Available options are:

	Nearest | Bilinear | CatmullRom | Mitchell | Lanczos3

	These are offered by the fast_image_resize crate
	(https://docs.rs/fast_image_resize/2.5.0/fast_image_resize/). 'Nearest' is
	what I recommend for pixel art stuff, and ONLY for pixel art stuff. It is
	also the fastest filter.

	For non pixel art stuff, I would usually recommend one of the last three,
	though some experimentation will be necessary to see which one you like
	best.

	Default is Lanczos3.

*-o*, *--outputs*
	Comma separated list of outputs to display the image at. Use *swww query* to
	know which outputs are currently being used.

	If it isn't set, the image is displayed on all outputs.

*-t*, *--transition-type* <TRANSITION_TYPE>
	[Environment Variable $SWWW_TRANSITION]
	Sets the type of transition. Default is 'simple', that fades into the new
	image.

	Possible transitions are:

[- simple
:- left 
:- right 
:- top 
:- bottom 
:- wipe 
:- grow 
:- center 
:- any 
:- outer 
:- random

	The 'left', 'right', 'top' and 'bottom' options make the transition	happen
	from that position to its oposite in the screen.

	'wipe' is similar to 'left' but allows you to specify the angle for
	transition with the `--transition-angle` flag.

	'wave' is similar to 'wipe' but the sweeping line is wavy. You can control
	the "waviness" with `--transition-wave`.

	'grow' causes a growing circle to transition across the screen and allows
	changing the circle's center position with the `--transition-pos` flag.

	'center' is an alias to 'grow' with position set to center of screen.

	'any' is an alias to 'grow' with position set to a random point on screen.

	'outer' is the same as grow but the circle shrinks instead of growing.

	Finally, 'random' will select a transition effect at random

*--transition-step* <0-255>
	[Environment Variable $SWWW_TRANSITION_STEP]
	How fast the transition approaches the new image.
	
	The transition logic works by adding or subtracting from the current rgb
	values until the old image transforms in the new one. This controls by how
	much we add or subtract.

	For example, if pixel A is 000010, and we need it to transition to pixel B,
	which is 000020, if *transition-step* is 2, then in one frame pixel A will
	turn to 000012, in the next frame to 000014, and so on.
	
	Larger values will make the transition faster, but more abrupt. A value of
	255 will always switch to the new image immediately.

*--transition-duration* <seconds (can have decimals)>
	How long the transition takes to complete, in seconds.

	Note this doesn't work with the 'simple' transition.

*-h*, *--help*
	Print help (see a summary with '-h')

# DESCRIPTION
Sends an image (or animated gif) for the daemon to display. You can also use `-`
to read from stdin instead.

The images sent will be cached at _$XDG_CACHE_HOME/swww_ or _$HOME/.cache/swww_
if $XDG_CACHE_HOME does not exist. For each monitor, there will be a file in
those locations corresponding to the current image/animation being displayed.
Next time you run `swww init`, the `swww-daemon` will load those files.

# SEE ALSO
*swww-daemon*(1) *swww-query*(1)
