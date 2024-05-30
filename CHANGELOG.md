### Unreleased


### 0.9.5

This is mostly just fixes and small improvements.

#### Fixes

  * fixed wallpaper never setting `configured` to 'true'
  * fixed fractional scaling rounding incorrectly 
  * fixed scaling for vertical monitors (thanks, @AhJi26)
  * fixed the annoying black screen on login issue (finally)

#### Additions

  * add --no-cache option to `swww-daemon`, by @lucasreis1

#### Internal improvements

  * specialized transition for `--transition-type none` (previously it was an alias
  to `simple` with special values)
  * remove an extra call to `thread::sleep` when loading the cache
  * no longer using an `event_fd` to wake up the main thread

### 0.9.4

Fractional Scaling is finally implemented! Woooo!!

That also fixes a nasty problem some people were having with fractionally
scaled outputs.

Apologies for spaming small releases like these in short order. Hopefully this
will be the last one for a while.

### 0.9.3

Quick release to fix a scaling error that might affect a lot of people.

#### Fixes

  * fix wrong scale calculation

#### Internal Improvements

  * deleted leftover `/proc` traversal code in the client
  * no longer setting nonblocking mode for daemon socket, since we are already
  polling it
  * better IPC structs between client and daemon

### 0.9.2

#### Fixes

  * fix stack overflow on some systems, by @iynaix
  * make sure we start the daemon even when the socket file already exists
  * fix build in 32bit x86, by @Calandracas606
  * fix image resize when image is larger than monitor
  * fix transitions performance that had regressed from versions 0.8.*
  * added SIGHUP to the list of signals we catch to exit properly
  * allow `swww` to run on a nested wayland environment, by @Fuyukai.

#### Improvements

  * we no longer traverse `/proc` to detect whether the daemon is running, we
  just try pinging it instead, by @Fuyukai
  * many internal refactors:
    - client now sends images in the format requested by the daemon (previously
    we were transforming the images in the daemon itself)
    - simplified the daemon's transitions
    - using `bitcode` instead of `rkyv` for serialization
    - using `rustix` instead of `nix` for unix stuff 
    - The Big One: we've eliminated our dependency on `smithay-client-toolkit`,
    now make calls directly to `wayland-client`. This gives us more control over
    our code for the price of a little extra verbosity.


### 0.9.1

My bad everyone, `0.9.0` wasn't loading the cache, so I am publishing this quick
fix.

### 0.9.0

#### BREAKING CHANGES

MSRV is now 1.74.0.

#### Deprecated

`swww init` is now considered deprecated. Use `swww-daemon` instead. To run it
in the background, simply do `swww-daemon &`.

#### Fixes

  * fix the `let_underscore_lock` error. Note that all 0.8.* will probably no
  longer build with newer Rust versions due to that error. By @akida32
  * fixed webp and gifs that are only a static image
  * fixed busy waiting for WlBuffers to be released. This was a big one, and it
  involved rewriting a ton of stuff. We've implemented our own memory pool and
  are using frame callbacks to know when to draw now. A big thanks to @YaLTeR,
  @jeLee6gi for their patience and help with debugging and testing this thing.
  * properly removing all cache contents on `clean-cache`
  * always center images that are larger than the monitor
  * animations no longer overlap when sending two animated images in succession
  * fix randomize script trying to use directories as images. Fix was suggested
  by @MRSS02
  * waiting for child swww process when loading the cache, preventing zombie
  processes
  * waiting for daemon initialization before certain requests. By @musjj

#### Improvements

  * New, better compression function implementations. We are now using some
  SIMD code to accelerate the frame compression functions, leading to some nice
  speedups in some cases. Thanks to @Akida31 for their help in verying my unsafe
  code.
  * We are using 3 channel color formats for some nice perf and memory
  improvements. Unfortunately, it seems to not work for some people on some
  notebooks (see [Known Issues](#known-issues)).
  * Implemented a way to force the use of a specific wayland_shm format, as a
  workaround for [Known Issues](#known-issues). Also went ahead and implemented
  some cli options for the daemon.
  * Support for animated pngs
  * Support for animations when piping images from standard input
  * Fps is now a `u16`, so we can support newest monitors framerates
  * Created a `restore` command to manually restore the cache. By @musjj
  * GitHub actions! Big thanks to @MichaelOultram!

#### Known Issues

Some people are having some problems with the 3 channel color formats (see issue
#233). Currently, initializing the daemon with `swww-daemon --format xrgb` is a
workaround to that.

### 0.8.2

NOTE ALL 0.8.* VERSIONS WILL PROBABLY NO LONGER BUILD WITH NEWER RUST VERSIONS.
This is because Rust promoted `let_underscore_lock` to a hard error, which
wasn't the case at the time we've published these versions.

#### **ATTENTION PACKAGE MAINTAINERS** 

I've changed my username from `Horus645` to `LGFae`. This means you will have to
update the remote url to `https://github.com/LGFae/swww`. Anyone who also has
direct links to the old address should update them.

I've done this because mostly to make it more professional looking. I've
considered using two github accounts instead, but that would involve setting up
multiple ssh keys on my machine and  included other complications. I deeply
apologize for the inconvenience.

#### Changes:

  * update MSRV in README.md, by @micielski
  * implemented a `--no-cache` flag for `swww init`
  * fixes to the build script, by @m4rch3n1ng
  * client waits for daemon to be ready on `swww init`
  * more accurate image fit implementation
  * added MSRV to Cargo.toml, by @akida31
  * fix some documentation typos and inacuracies
  * fix timings for transition frames
  * fix capacity of Vec in `image_pad` function, by @MichaelOultram
  * implement animated WebP Support, by @MichaelOultram
  * some other memory optimizations, by @MichaelOultram
  * implement `clear-cache` command
  * we also automatically clear the cache from old `swww` versions now!

Also, we have udpdated all crates versions so that they all match. This should
help some package maintainers that were having difficulty setting `swww` up for
e.g. Debian, I believe.


### 0.8.1

Pretty a much a near-exclusive bug fix release:

  * Fixed `swww clear` causing the daemon to exit
  * The cache is once again being correctly load during `swww init`
  * Fixed glitches happening to animated gifs (frames were being loaded in the 
  wrong order)
  * Fixed `swww-daemon` sometimes not drawing to the whole screen (forgot to set
  the exclusive zone to -2)
  * Fixed an issue where the daemon would hang if multiple images were sent in
  quick succession

#### Additions:

  * `--transition-type none`, which is an alias to `--trasition-type simple`
  `--transition-step 255`


### 0.8.0

#### BREAKING CHANGE: CACHE HAS CHANGED:

I have changed the way we are caching the images / animations. **I would
recommend users to delete the previous cache directory once they install this
new version**:

```bash
rm -r $XDG_CACHE_HOME/swww # OR
rm -r $HOME/.cache/swww
```

#### BREAKING CHANGE: NUKED `--sync` flag:

We now sync everything automatically, every time. So we've eliminated that flag.

#### Update to [`sctk 0.17`](https://github.com/Smithay/client-toolkit):

Updating to [`sctk 0.17`](https://github.com/Smithay/client-toolkit) unlocked
many, many improvements:
  * We managed to ditch and extra `clone` when calculating the transitions
  * We now draw the images directly into the wayland buffer, instead of having
  to send an intermidiary buffer through a channel.
  * Because of the above, we can now send from the client a `bgr` image, instead
  of a `bgra` one. This lets us seriaze and write roughly 3/4 of what we were
  doing previously.

#### Moving from `serde` to `rkyv`:

We have changed our serialization strategy from
[`serde`](https://github.com/serde-rs/serde) to
[`rkyv`](https://github.com/rkyv/rkyv). This lead to even further memory usage
reductions, since `rkyv` does not use an intermidate buffer to deserialize its
structures.

#### Additions:

We have reworked the way we do synchronization between monitors. It should sync
all monitors animations automatically.

  * New transition, `fade`, that is essentially `simple` with beziers (@flick0)
  * `invert_y` flag (@flick0)
  * New option to resize to fit, padding only what was left (@SignalWalker)
  * @RiedleroD fixed a type :)

#### Summary:

With all of the changes above, **I've managed to reduce memory consumption
almost by a factor of 3**. The price to pay was a full rewrite of the wayland
implementation part of the daemon, a partial rewrite of the way we do ipc,
and all the code adaptions necessary to make all that work.

Unfortunately, because I had to rewrite so much stuff, it is possible that old
bugs will resurface. I've tried my best to test and validate it with every thing
that blew up in the past, but it is probably inevitable that some stuff slipped
by. Apologies in advance, and keep this in mind when upgrading.

### 0.7.3

Fixes:
  * Missing `/` when using `$HOME/.cache/swww`, by @max-ishere
  * `--transition-step` with `simple` has saner defaults
  * correctly splitting outputs argument with ',', by @potatoattack

Improvements:
  * we only send the image after we've finished processing the whole animation.
  This diminishes the weird lag that can sometimes happen when sending a large
  gif.
  * sending a status update to systemd when daemon has initialized, by @b3nj4m1n

Internals:
  * we have benchmarks for our compression functions! This will be useful for
  later once I start poking things to see if I can make them more efficient.

Unfortunately, there are a few bugs people have reported that I still haven't
been able to fix. Sorry about that.

In any case, the immediate plan for the future is actually to update sctk to
version 0.17.0. I am actually studying the possibility of using sctk with wgpu,
since that should bring many improvements (most noticeably, we would be able to
store an rgb vector, instead of rgba, so we could potentially cut memory usage
by 3/4. In theory, of course, I still have to actually measure it to see if
there's any difference. It will probably still take a while).

### 0.7.2

Improvements:
  * Images (and animations) are now cached at `$XDG_CACHE_HOME` #65
  * We now have man-pages! They must be installed manually in your system by
    moving the files in `doc/generated` to the appropriate location. Typically,
	you can figure out where that is by running `manpath`. The script
	`doc/gen.sh` will create the above directory and all the relevant manpages.
	Note that the script depends on `scdoc` being installed in the system.
	Have a look at it for more details.
  * We now also have automated spell checking. This let us fix a number in typos
    in our documentation, both internal and user-oriented.
  * New option for `swww-img`: `--sync`. This syncs the animations in all your
  monitors. Note that *all monitors must be displaying animations* in order for
  it to work.

Internal:
  * Integration tests are not run by default. You must now use
  `cargo test -- --ignored` to run them. This will make it possible for some
  people (like the ones trying to package `swww` at Nix) to run some of the
  tests in a sandboxed environment where they don't have access to the wayland
  server. If anyone is interested in running *all* tests, they can do that with
  `cargo test -- --include-ignored`.

### 0.7.1

Improvements:
  * you can now use absolute screen coordinates with `--transition-pos`
  (@flick0)

Fixes:
  * `swww query` not returning the image being displayed
  * document `--no_resize` and `--fill_color` options for `swww img`
  * reading img from stdin (now with a proper integration test to make sure
  it doesn't happen again) (#42)

Internal:
  * fixed `tests/integration_tests.rs` calling the wrong `swww-daemon` binary

### 0.7.0

**BREAKING CHANGES**

  * **ATTENTION, PACKAGE MAINTAINERS** - `swww` is now composed of two separate
  binaries: `swww` and `swww-daemon`. **Both** must be installed on the user's
  system in order for `swww` to work correctly. Doing this allowed for major
  improvements in terms of overall memory usage, among other things (#52).

Improvements:
 
 * separate client and daemon (see above).
 * we don't try to animate `gif` files that have only one frame
 * we can read images from stdin (not this does not work for animated gifs; we
 simply display the image's first frame) (#42)
 * `--no-resize` option (pads the outer part of the image with `fill-color`)
 (#37)
 * new transition: `wave`, by @flick0
 * reading image format properly (instead of using file extension) (#74)

Fixes:
  * fixed panic with on gif that had identical frames (#68)
  * fixed panic with fractional-scaling (#73) (by @thedmm)

Non-breaking Changes:
 * @flick0 changed the default `transition-step`


Internal:
 * Many improvements to the README.md (@aouerfelli and @flick0)


### 0.6.0

**BREAKING CHANGES**

  * `transition-speed` no longer exists. Now, speed is controlled through a
  bezier curve (`transition-bezier`), and duration (`transition-duration`)
  flags (note this also applies to the env var, SWWW_TRANSITION_SPEED). A
  warning was added when we detect the presence of the SWWW_TRANSITION_SPEED
  environment variable. This warning will go away in the next release, it is
  only there as a means of making sure everyone knows the variable has been
  superseded, and having it in your configs no longer does anything.

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

  * Check to see if daemon is REALLY running when we see that socket already
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
* Bumped rust edition to 2021
* Our custom compression is now even faster
* I did a rewrite of the way the code that handled animations was structured.
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
