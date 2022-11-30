### Unreleased


### 0.6.0

**BREAKING CHANGES**

  * `transition-speed` no longer exists. Now, speed is controlled through a
  bezier curve (`transition-bezier`), and duration (`transition-duration`)
  flags (note this also applies to the env var, SWWW_TRANSITION_SPEED). A
  warning was added when we detect the presence of the SWWW_TRANSITION_SPEED
  environment variable. This warning will go away in the next release, it is
  only there as a means of making sure everyone knows the variable has been
  superseeded, and having it in your configs no longer does anything.

Improvements:

  * New grow transition. Grow and outer transition now accept a --transition-pos
  command line argument. By @flick0.
  * Transitions `grow` and `outer` now both work with bezier curves (see
  breaking changes, above). This allows for finer control in animation speed
  than before. Also by @flick0.
  * Very slightly faster decompression routine

### 0.5.0

**BREAKING CHANGES**:

  * `swww query` now formats its output as `<output>: ...`, instead of
  `<output> = ...`. This will break your scripts if you relied on the output's
  format.

Improvements:

  * Fixed `swww` getting stuck on a futex when a new monitor was connected (#26)
  * New `wipe` transition by @flick0
  * Several small code improvements by @WhyNotHugo
  * Typo fix (@thebenperson)

### 0.4.3

  * Check to see if daemon is REALLY running when we see tha socket already
  exists (#11)
  * Fix dpi scaling (#22)

### 0.4.2

  * Fixed #13.
  * Improved error message when daemon isn't running (#18)

### 0.4.1

  * Fixed regression where the image was stretched on resize (#16)

### 0.4.0

  * implemented the new transition effects

  * refactored socket code

  * refactored event loop initialization code, handling errors properly now

  * BREAKING CHANGE: we are using fast_image_resize to resize our images now.
  This makes resizing much faster (enough to smoothly play animations before
  caching is done), but it makes it so that the `Gaussian` and `Triangle`
  filters no longer exist. Furthermore, the filters `Bilinear` and `Mitchell`
  were added.

  * deleted previously deprecated `init -i` and `init -c` options

### 0.3.0

* Limited image formats to: `gif`, `jpeg`, `jpeg_rayon`, `png`, `pnm`, `tga`,
  `tiff`, `webp`, `bmp`, `farbfeld`
* Bumbed rust edition to 2021
* Our custom compression is now even faster
* I did a rewrite of the way the code that handled animations was structed.
  This made caching a LOT faster, but it incurs in more memory usage, since
  we spawn an extra thread to make a pipeline. That said, since this also 
  greatly simplified the code itself, I considered it an overall positive
  change.
* Fixed a bug where the animation wouldn't stop until it had processed all the
  frames, even when it was told to.
* Setting a custom names and stack sizes to our threads. The custom name will
  help in debugging in the future, and the custom stack sizes lets us push the
  memory usage even lower.
* Did all the preparatory work for us to start writing new transition effects.
  Ideally they should come in the next version, which should hopefully also be
  our first release (since then I will consider swww to be pretty much feature
  complete).

### 0.2.0

Using unsafe to speed up decompression.
Also, `swww init -i` and `swww init -c` may now be considered deprecated.
It was originally created to bypass `swww init && swww img <path/to/img>` not
working. Now, however, it seems to be working properly. In hindsight, it was
probably already working for a while, but I failed to test it properly and
thought it was still a problem.

The `swww init -i` and `swww init -c` options shall remain for now, for 
compatibility and just in case a regression happens. Once I am confident
enough, they will be eliminated (that will let me erase around 50 lines of
code, I think).

### 0.1.0

Initial release.
