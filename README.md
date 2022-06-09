# A Solution to your Wayland Wallpaper Woes
### Efficient animated wallpaper daemon for wayland, controlled at runtime

## Dependencies

 - a compositor that implements:
   * wlr-layer-shell
   * xdg-output
   * xdg-shell
 - lz4 (for compressing frames when animating)

## Build

### Dependencies:

  - Up to date stable rustc compiler and cargo

To build, clone this repository and run:
```
cargo build --release
```
Then, put the binary at `target/release/swww` in your path. Optionally,
autocompletion scripts for bash, zsh, fish and elvish are offered in the
`completions` directory.

## Features

 - Display animated gifs on your desktop
 - Display any image in the formats:
   * jpeg
   * png
   * gif
   * pnm
   * tga
   * tiff
   * webp
   * bmp
   * farbfeld
 - Clear the screen with an arbitrary rrggbb color
 - Smooth transition effect when you switch images
 - Do all of that without having to shutdown and reinitialize the daemon

## Why

There are two main reasons that compelled me to make this, the first, that
[oguri](https://github.com/vilhalmer/oguri) hasn't updated in over a year as I
am writting this (02 Feb 2022), despite there being serious problems with
excess of memory use while displaying certain gifs (see
[this](https://github.com/vilhalmer/oguri/issues/38),for example). The best
alternative I've found for oguri was
[mpvpaper](https://github.com/GhostNaN/mpvpaper), but if felt like quite the
overkill for my purposes.

Comparing to `oguri`, `swww` uses less cpu power to animate once it has cached
all the frames in the animation. It should also be **significantly** more
memory efficient (make sure to see the
[Caveats/Limitations](#CaveatsLimitations) though).

The second is that, to my knowledge, there is no wallpaper daemon for wayland
that allows you to change the wallpaper at runtime. That is, is order to, for
example, cycle through the images of a directory, you'd have to kill the daemon
and restart it. Not only does it make simple shell scripts a pain to write, it
makes switching from one image to the next to happen very abruptly.

## Usage

Start by initializing the daemon:
```
swww init
```
Then, simply pass the image you want to display:
```
swww img <path/to/img>

# You can also specify outputs:
swww img -o <outputs> <path/to/img>

# Control how smoothly the transition will happen and/or it's frame rate
# For the step, smaller values = more smooth. Default = 20
# For the frame rate, default is 30.
swww img --transition-step <1 to 255> --transition-fps <1 to 255>

# Note you may also control the above by setting up the SWWW_TRANSITION_FPS and
# SWWW_TRANSITION_STEP environment variables.
```
If you would like to know the valid values for *\<outputs\>*, you can query the
daemon. This will also tell you what the current image being displayed is, as
well as the dimensions detected for the outputs. If you need more detailed
information, I would recommend using
[wlr-randr](https://sr.ht/~emersion/wlr-randr/).
```
swww query
```
Finally, to stop the daemon, kill it:
```
swww kill
```
For a more complete description, run *swww --help* or *swww \<subcommand\>
--help*.

Finally, to get a feel for what you can do with some shell scripting, check out
the 'example_scripts' folder. It can help you get started.

## Caveats/Limitations

I had a glorious name when I started this project, but alas, I couldn't quite
get there, here are some issues with it:

 - Despite trying my best to make this as resource efficient as possible,
 **memory usage seems to increase a little bit with every new image openned**.
 Strangenly, it also seems that openning the same image again will *not*
 increase usage further.  It shouldn't be a big issue unless you want to go
 through all images in a huge directory (say, 100+ images). Note that, after
 going through it once, memory usage should more or less stabilize. See
 https://github.com/Horus645/swww/issues/4 for some extra details.
 - If the daemon exits in an unexpected way (for example, if you send SIGKILL to
 force its shutdown), it will leave a `swww.socket` file behind in
 `$XDG_RUNTIME_DIR` (or `/tmp/swww` if it isn't set). If you want to
 reinitialize the daemon, you will have to remove that file first.

## About new features

Broadly speaking, **NEW FEATURES WILL NOT BE ADDED, UNLESS THEY ARE EGREGIOUSLY
SIMPLE**. I made `swww` with the specific usecase of making shell scripts in
mind. So, for example, stuff like timed wallpapers, or a setup that loads a
different image at different times of the day, and so on, should all be done by
combining `swww` with other programs (see the 'example_scripts' for some
examples).

If you really want some new feature within `swww` itself, I would recommend
forking the repository.

That said, I have plans for adding some different transition effects. If you
happen to have ideas feel free to open an issue (or maybe even making a PR)!

## Alternatives

`swww` isn't really the simplest, mostest minimalest software you could find
for managing wallpapers. If you are looking for something simpler, have a look
at the [awesome-wayland repository list of wallpaper programs
](https://github.com/natpen/awesome-wayland#wallpaper). I can personally
recommend:

 - [wbg](https://codeberg.org/dnkl/wbg) - probably the simplest of them all.
 Strongly recommend if you just care about setting a single png as your
 permanent wallpaper on something like a laptop.
 - [swaybg](https://github.com/swaywm/swaybg) - made by the wlroots gods
 themselves.
 - [mpvpaper](https://github.com/GhostNaN/mpvpaper) - if you want to display
 videos as your wallpapers. This is also what I used for gifs before making
 `swww`.

## Acknowledgments

A huge thanks to everyone involed in the [smithay](https://github.com/Smithay)
project. Making this program would not have been possible without it. In fact,
the first versions of swww were quite literaly copy pasted from the [layer shell
example in the client-toolkit
](https://github.com/Smithay/client-toolkit/blob/master/examples/layer_shell.rs).
