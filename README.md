# A Solution to your Wayland Wallpaper Woes
### Efficient animated wallpaper daemon for wayland, controlled at runtime

![animated gif demonstration](https://i.imgur.com/Leuh6wm.gif)
![image transition demonstration](../demos/assets/grow.gif)

## Dependencies

 - a compositor that implements:
   * wlr-layer-shell (typically wlroots based compositors)
   * xdg-output
 - [lz4](https://github.com/lz4/lz4) (for compressing frames when animating)

**Note that this means `swww` will not run on Gnome, because it does not implement the `wlr-layer-shell` protocol**.

## Build

<a href="https://repology.org/project/swww/versions">
    <img src="https://repology.org/badge/vertical-allrepos/swww.svg" alt="Packaging status" align="right">
</a>

### Dependencies:

  - Up to date stable rustc compiler and cargo (specifically, MSRV is 1.74.0)

To build, clone this repository and run:
```
cargo build --release
```
Then, put **both binaries** `target/release/swww` and
`target/release/swww-daemon` in your  path. Optionally, autocompletion scripts
for bash, zsh, fish and elvish are offered in the `completions` directory.

#### Man pages:

In order to generate the man pages, **you must have `scdoc` installed**. Run

```
./doc/gen.sh
```

The man pages will be in `doc/generated`. To install them, you must move them to
to the appropriate location in your system. You should be able to figure out
where that is by running `manpath`.

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

There are two main reasons that compelled me to make this: the first is that
[`oguri`](https://github.com/vilhalmer/oguri) is unmaintained and archived,
despite there being serious problems with excess of memory use while displaying
certain gifs (see [this](https://github.com/vilhalmer/oguri/issues/38), for
example). The best alternative I've found for `oguri` was
[`mpvpaper`](https://github.com/GhostNaN/mpvpaper), but if felt overkill for my
purposes.

Comparing to `oguri`, `swww` uses less cpu power to animate once it has cached
all the frames in the animation. It should also be **significantly** more
memory efficient.

The second is that, to my knowledge, there is no wallpaper daemon for wayland
that allows you to change the wallpaper at runtime. That is, in order to, for
example, cycle through the images of a directory, you'd have to kill the daemon
and restart it. Not only does it make simple shell scripts a pain to write, it
makes switching from one image to the next to happen very abruptly.

## Usage

Start by initializing the daemon:
```
swww-daemon
```
Then, in a different terminal, simply pass the image you want to display:
```
swww img <path/to/img>

# You can also specify outputs:
swww img -o <outputs> <path/to/img>

# Control how smoothly the transition will happen and/or it's frame rate
# For the step, smaller values = more smooth. Default = 20
# For the frame rate, default is 30.
swww img <path/to/img> --transition-step <1 to 255> --transition-fps <1 to 255>

# There are also many different transition effects:
swww img <path/to/img> --transition-type center

# Note you may also control the above by setting up the SWWW_TRANSITION_FPS,
# SWWW_TRANSITION_STEP, and SWWW_TRANSITION environment variables.

# To see all options, run
swww img --help
```
If you would like to know the valid values for *\<outputs\>*, you can query the
daemon. This will also tell you what the current image being displayed is, as
well as the dimensions detected for the outputs. If you need more detailed
information, I would recommend using
[`wlr-randr`](https://sr.ht/~emersion/wlr-randr/).
```
swww query
```
Finally, to stop the daemon, kill it:
```
swww kill
```
For a more complete description, run `swww --help` or `swww <subcommand>
--help`.

Finally, to get a feel for what you can do with some shell scripting, check out
the [example_scripts](/example_scripts/) folder. It can help you get started.

## Transitions

#### Example wipe transition:

> wipe transition with angle set to 30 deg

![top transition demonstration](../demos/assets/wipe.gif)

The `left`, `right`, `top` and `bottom` transitions all work similarly.

#### Example outer transition

![outer transition demonstration](../demos/assets/outer.gif)

The `center` transition is the opposite: it starts from the center and goes 
towards the edges.

There is also `simple`, which simply fades into the new image, `any`, which 
starts at a random point with either `center` of `outer` transitions, and `random`,
which selects a transition effect at random.

## Troubleshooting

### The image looks tilted and in grayscale on my laptop

See #233. Current workaround is to use `swww-daemon --format xrgb` when starting
the daemon.

### High cpu usage during caching of a gif's frames

`swww` will use a non-insignificant amount of cpu power while caching the
images. This will be specially noticeable if the images need to be resized
before being displayed. So, if you have a very large gif, I would recommend
resizing it **before** sending it to `swww`. That would make the caching phase
much faster, and thus ultimately reduce power consumption. I can personally
recommend [`gifsicle`](https://github.com/kohler/gifsicle) for this purpose.

### Wallpaper disappears when reconnecting monitor

`swww` used to cache its images so that it could reload the current the last
displayed image automatically. This lead to many problems and also proved to be
very annoying to keep working with when we updated to
[`sctk 0.17`](https://github.com/Smithay/client-toolkit). So I decided to nuke
it.

If you want a wallpaper to be set automatically when you reconnect to a monitor,
you should use a combination of scripts and a program that lets you run commands
when a new output is connected, like [`kanshi`](https://sr.ht/~emersion/kanshi/).

## About new features

Broadly speaking, **NEW FEATURES WILL NOT BE ADDED, UNLESS THEY ARE EGREGIOUSLY
SIMPLE**. I made `swww` with the specific usecase of making shell scripts in
mind. So, for example, stuff like timed wallpapers, or a setup that loads a
different image at different times of the day, and so on, should all be done by
combining `swww` with other programs (see the [example_scripts](/example_scripts/) for some
examples).

If you really want some new feature within `swww` itself, I would recommend
forking the repository.

## Alternatives

`swww` isn't really the simplest, mostest minimalest software you could find
for managing wallpapers. If you are looking for something simpler, have a look
at the [awesome-wayland repository list of wallpaper programs
](https://github.com/natpen/awesome-wayland#wallpaper). I can personally
recommend:

 - [`wbg`](https://codeberg.org/dnkl/wbg) - probably the simplest of them all.
 Strongly recommend if you just care about setting a single png as your
 permanent wallpaper on something like a laptop.
 - [`swaybg`](https://github.com/swaywm/swaybg) - made by the wlroots gods
 themselves.
 - [`mpvpaper`](https://github.com/GhostNaN/mpvpaper) - if you want to display
 videos as your wallpapers. This is also what I used for gifs before making
 `swww`.

## Acknowledgments

A huge thanks to everyone involved in the [smithay](https://github.com/Smithay)
project. Making this program would not have been possible without it. In fact,
the first versions of swww were quite literally copy pasted from the
[layer shell example in the client-toolkit
](https://github.com/Smithay/client-toolkit/blob/master/examples/layer_shell.rs).

A big thank-you also to [HakierGrzonzo](https://github.com/HakierGrzonzo), for
setting up the AUR package.

### Wallpapers used in this README

Pixel Art, by Waneella - https://www.patreon.com/waneella

Gradient - https://www.behance.net/gallery/86128681/Free-Unicorn-Vector-Gradients

Silhouette of Skyway - https://unsplash.com/photos/silhouette-of-skyway-UUJzCuHUfYI
