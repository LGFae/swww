# A Solution to your Wayland Wallpaper Woes
### Efficient animated wallpaper daemon for wayland, controlled at runtime

![animated gif demonstration](https://i.imgur.com/Leuh6wm.gif)
![image transition demonstration](https://i.imgur.com/fMBFruY.gif)

## Dependencies

 - a compositor that implements:
   * wlr-layer-shell
   * xdg-output
 - lz4 (for compressing frames when animating)

## Build

<a href="https://repology.org/project/swww/versions">
    <img src="https://repology.org/badge/vertical-allrepos/swww.svg" alt="Packaging status" align="right">
</a>

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
memory efficient.

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
swww img <path/to/img> --transition-step <1 to 255> --transition-fps <1 to 255>

# There are also many different transition effects:
swww img <path/to/img> --transition-type center

# Note you may also control the above by setting up the SWWW_TRANSITION_FPS,
# SWWW_TRANSITION_STEP, and SWWW_TRANSITION_TYPE environment variables.

# To see all options, run
swww img --help
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

## Transitions

#### Example top transition:

![top transition demonstration](https://i.imgur.com/ULm6XWI.gif)

The `left`, `right` and `bottom` transitions all work similarly.

#### Example outer transition

![outer transition demonstration](https://i.imgur.com/o4pSyxW.gif)

The `center` transition is the opposite: it starts from the center and goes 
towards the edges.

There is also `simple`, which simply fades into the new image, `any`, which is
like `center` but starts at a random point, and `random`, which selects a
transition effect at random.

## Troubleshooting

#### High cpu usage during caching of a gif's frames

`swww` will use a non-insignificant amount of cpu power while caching the
images. This will be specially noticeable if the images need to be resized
before being displayed. So, if you have a very large `gif`, I would recommend
resizing it **before** sending it to `swww`. That would make the caching phase
much faster, and thus ultimately reduce power consumption. I can personally
recommend [gifsicle](https://github.com/kohler/gifsicle) for this purpose.

## About new features

Broadly speaking, **NEW FEATURES WILL NOT BE ADDED, UNLESS THEY ARE EGREGIOUSLY
SIMPLE**. I made `swww` with the specific usecase of making shell scripts in
mind. So, for example, stuff like timed wallpapers, or a setup that loads a
different image at different times of the day, and so on, should all be done by
combining `swww` with other programs (see the 'example_scripts' for some
examples).

If you really want some new feature within `swww` itself, I would recommend
forking the repository.

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

A big thank-you also to [HakierGrzonzo](https://github.com/HakierGrzonzo), for
setting up the AUR package.
