# The Final Solution to your Wayland Wallpaper Woes
### Efficient animated wallpaper daemon for wayland, controlled at runtime

## Dependencies

 - a compositor that implements:
   * wlr-layer-shell
   * xdg-output
   * xdg-shell

## Build

### Dependencies:

  - Up to date stable rustc compiler and cargo

To build, clone this directory and run:
```bash
cargo build --release
```
Then, put the binary at `target/release/fswww` in your path.
Optionally, autocompletion scripts for bash, zsh, fish and elvish are offered
in the `completions` directory.

## Features

 - Display animated gifs on your desktop
 - Display any image in a format that is decodable by the [image](https://github.com/image-rs/image#supported-image-formats) crate.
 - Clear the screen with an arbitrary rrggbb color
 - Smooth transition effect when you switch images

## Why

There are two main reasons that compelled me to make this, the first, that
[oguri](https://github.com/vilhalmer/oguri) hasn't updated in over a year as I
am writting this (02 Feb 2022), despite there being serious problems with
excess of memory use while displaying certain gifs (see [this](https://github.com/vilhalmer/oguri/issues/38),
for example). The best alternative I've found for oguri was [mpvpaper](https://github.com/GhostNaN/mpvpaper), 
but if felt like quite the overkill for my purposes.

Comparing to `oguri`, `fswww` uses less cpu power to animate once it has cached
all the frames in the animation. It should also be **significantly** more
memory efficient (make sure to see the 
[Caveats/Limitations](#CaveatsLimitations) though).

The second is that, to my knowledge, there is no wallpaper daemon for wayland
that allows you to change the wallpaper at runtime. That is, is order to, for
example, cycle through the images of a directory, you'd have to kill the daemon
and restart it. Not only does it make simple shell scripts a pain to write, it
makes switch from one image to the next to happen very abruptly.

## Usage

Start by initializing the daemon:
```bash
fswww init
```
Then, simply pass the image you want to display:
```bash
fswww img <path/to/img>

# You can also specify outputs:
fswww img -o <outputs> <path/to/img>

# Control how smoothly the transition will happen and/or it's frame rate
# For the step, smaller values = more smooth. Default = 20
# For the frame rate, default is 30.
fswww img --transition-step <1 to 255> --transition-fps <1 to 255>
```
If you would like to know the valid values for *\<outputs\>* then you can query
the daemon. This will also tell you what the current image being displayed is,
as well as the dimensions detected for the outputs. If you need more detailed
information, I would recommend using [wlr-randr](https://sr.ht/~emersion/wlr-randr/).
```bash
fswww query
```
Finally, to stop the daemon, kill it:
```bash
fswww kill
```
For a more complete description, run *fswww --help* or *fswww \<subcommand\>
--help*.

## Caveats/Limitations

I had a glorious name when I started this project, but alas, I couldn't quite
get there, here are some issues with it:

 - To initialize the daemon already displaying and image, use:
 ```bash
 fswww init --img <path/to/img> # Do this
 ```
 Do **NOT** use something like:
 ```bash
 fswww init && fswww img <path/to/img> # Don't do this
 ```
 As that might straight up not work. In particular, it tends to fail when using
 it in a compositor's init script (which is probably where you will want to 
 `init` the daemon).
 - Despite trying my best to make this as resource efficient as possible,
 **memory can still be an issue**. From my testing, this seems to be mostly
 related to how images are loaded with the
 [image](https://github.com/image-rs/image#supported-image-formats) crate.
 Strangenly, it also seems that openning the same image again will *not*
 increase usage further. Still trying to understand what's going on here.
 - If the daemon exits in an unexpected way (for example, if you send SIGKILL to
 force its shutdown), it will leave a `fswww.socket` file behind in 
 `$XDG_RUNTIME_DIR` (or `/tmp/fswww` if it isn't set). If you want to 
 reinitialize the daemon, you will have to remove that file first.
