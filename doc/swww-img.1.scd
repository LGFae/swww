swww-img(1)

# NAME
swww-img

# SYNOPSIS
*swww img* [OPTIONS] <path/to/img>

# OPTIONS

*-f*, *--filter* <FILTER>
	Filter to use when scaling images

	Available options are:

	_Nearest_ | _Bilinear_ | _CatmullRom_ | _Mitchell_ | _Lanczos3_

	These are offered by the fast_image_resize crate
	(https://docs.rs/fast_image_resize/2.5.0/fast_image_resize/). _Nearest_ is
	what I recommend for pixel art stuff, and ONLY for pixel art stuff. It is
	also the fastest filter.

	For non pixel art stuff, I would usually recommend one of the last three,
	though some experimentation will be necessary to see which one you like
	best.

	Note you can also pass the flag *--no-resize*, explained below. In which
	case the *--filter* flag will have no effect.

	Default is Lanczos3.

*--no-resize*
	Do not resize the image. Equivalent to *--resize* _no_.

	If this is set, the image won't be resized, and will be centralized in the
	middle of the screen instead. If it is smaller than the screen's size, it
	will be padded with the value of *--fill_color*, below.

*--resize* <RESIZE>
	Whether to resize the image and the method by which to resize it.

	Possible values:
		- _no_:   Do not resize the image
		- _crop_: Resize the image to fill the whole screen, cropping out parts that don't fit
		- _fit_:  Resize the image to fit inside the screen, preserving the original aspect ratio

	Default is _crop_.

*--fill-color* <RRGGBB>
	Which color to fill the padding with when not resizing.

	Default is _000000_.

*-o*, *--outputs*
	Comma separated list of outputs to display the image at. Use *swww query* to
	know which outputs are currently being used.

	If it isn't set, the image is displayed on all outputs.

*-t*, *--transition-type* <TRANSITION_TYPE>
	\[Environment Variable $SWWW_TRANSITION]

	Sets the type of transition. Default is _simple_, that fades into the new
	image.

	Possible transitions are:

[- _none_
:- _simple_
:- _fade_
:- _left_
:- _right_
:- _top_
:- _bottom_
:- _wipe_
:- _wave_
:- _grow_
:- _center_
:- _any_
:- _outer_
:- _random_

	_none_ will complete the transition instantly.

	_fade_ is like _simple_ but uses bezier curves while fading the image, its a
	more polished looking version of _simple_ with less artifacts

	The _left_, _right_, _top_ and _bottom_ options make the transition	happen
	from that position to its opposite in the screen.

	_wipe_ is similar to _left_ but allows you to specify the angle for
	transition with the `--transition-angle` flag.

	_wave_ is similar to _wipe_ but the sweeping line is wavy. You can control
	the "waviness" with `--transition-wave`.

	_grow_ causes a growing circle to transition across the screen and allows
	changing the circle's center position with the `--transition-pos` flag.

	_center_ is an alias to _grow_ with position set to center of screen.

	_any_ is an alias to _grow_ with position set to a random point on screen.

	_outer_ is the same as grow but the circle shrinks instead of growing.

	Finally, _random_ will select a transition effect at random

*--transition-step* <0-255>
	\[Environment Variable $SWWW_TRANSITION_STEP]

	How fast the transition approaches the new image.

	The transition logic works by adding or subtracting from the current rgb
	values until the old image transforms in the new one. This controls by how
	much we add or subtract.

	For example, if pixel A is 000010, and we need it to transition to pixel B,
	which is 000020, if *transition-step* is 2, then in one frame pixel A will
	turn to 000012, in the next frame to 000014, and so on.

	Larger values will make the transition faster, but more abrupt. A value of
	255 will always switch to the new image immediately.

	Default is 90.
	If *transition-type* is _simple_, default is 2.

*--transition-duration* <seconds (can have decimals)>
	\[Environment Variable $SWWW_TRANSITION_DURATION]

	How long the transition takes to complete, in seconds.

	Note this doesn't work with the _simple_ transition.

	Default is 3.

*--transition-fps* <frames per second (max 255)>
	\[Environment Variable: $SWWW_TRANSITION_FPS]

	Frame rate for the transition effect.

	Note there is no point in setting this to a value smaller than what your
	monitor supports.

	Also note this is **different** from the transition-step. That one controls
	by how much we approach the new image every frame.

	Default is 30.

*--transition-angle* <angle, in degrees (parsed as a float)>
	\[Environment Variable: SWWW_TRANSITION_ANGLE]

	This is used for the _wipe_ and _wave_ transitions. It controls the angle of
	the wipe.

	Note that the angle is in degrees, where '0' is right to left and '90'
	is top to bottom, and '270' bottom to top

	Default is 45.

*--transition-pos* <x,y>
	\[Environment Variable: SWWW_TRANSITION_POS]

	This is only used for the _grow_ and _outer_ transitions. It controls the
	center of circle (default is _center_).

	Position values can be given in both percentage values and pixel values:
	float values are interpreted as percentages and integer values as pixel
	values. Eg.: 0.5,0.5 means 50% of the screen width and 50% of the screen
	height, while 200,400 means 200 pixels from the left and 400 pixels from the
	bottom.

	The value can also be an alias which will set the position accordingly:
[- _center_
:- _top_
:- _left_
:- _right_
:- _bottom_
:- _top-left_
:- _top-right_
:- _bottom-left_
:- _bottom-right_

	Default is _center_.

*--invert-y* <bool>
	\[Environment Variable: SWWW_INVERT_Y]

	inverts the y position sent in `transiiton_pos` flag

*--transition-bezier* <f1,f2,f3,f4 (all floats)>
	\[Environment Variable: SWWW_TRANSITION_BEZIER]

	Bezier curve to use for the transition animation. https://cubic-bezier.com
	is a good website to get these values from.

	eg: 0.0,0.0,1.0,1.0 for linear animation

	Default is .54,0,.34,.99

*--transition-wave* <width,height (both floats)>
	\[Environment Variable: SWWW_TRANSITION_WAVE]

	Currently only used for _wave_ transition to control the width and height of
	each wave.

	Default is : 20,20

*-h*, *--help*
	Print help (see a summary with '-h')

# DESCRIPTION
Sends an image (or animated gif) for the daemon to display. You can also use `-`
to read from stdin instead.

# ABOUT THE CACHE

The images sent will be cached at _$XDG_CACHE_HOME/swww_ or _$HOME/.cache/swww_
if $XDG_CACHE_HOME does not exist. For each monitor, there will be a file in
those locations corresponding to the current image/animation being displayed.
Importantly, **cache will only be loaded during initialization if you use swww
init**. That is, calling `swww-daemon` directly will **NOT** load the cache, but
calling `swww-init` will.

The `swww-daemon` will actually wait until the first image has been set before
trying to load the cache.

Finally, the cache will keep preprocessed versions of `gif`s. So, if you load a
large `gif`, you would have to pay the price for its processing the first time.
If you constantly load large `gif`s, this could cause the cache to get very big.
You can simply run `swww clean-cache` if this happens.

# SEE ALSO
*swww-clear-cache*(1) *swww-daemon*(1) *swww-query*(1)
