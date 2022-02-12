# The Final Solution to your Wayland Wallpaper Woes
### Efficient animated wallpaper daemon for wayland, controlled at runtime

#### Dependencies

 - a compositor that implements:
   * wlr-layer-shell
   * xdg-output
   * xdg-shell

#### Build

##### Dependencies:

  - Up to date stable rustc compiler and cargo

To build, clone this directory and run:
```
cargo build --release
```
Then, put the binary at `target/release/fswww` in your path.
Optionally, autocompletion scripts for bash, zsh, fish and elvish are offered
in the `completions` directory.

#### Features

 - Display animated gifs on your desktop
 - Display any image in a format that is decodable by the [image](https://docs.rs/image/latest/image/codecs/index.html#supported-formats) crate.
 - Clear the screen with an arbitrary rrggbb color
 - Smooth transition effect when you switch images

#### Why

There are two main reasons that compelled me to make this, the first, that
[oguri](https://github.com/vilhalmer/oguri) hasn't updated in over a year as I
am writting this (02 Feb 2022), despite there being serious problems with
excess of memory use while displaying certain gifs (see [this](https://github.com/vilhalmer/oguri/issues/38),
for example). The best alternative I've found for oguri was [mpvpaper](https://github.com/GhostNaN/mpvpaper), 
but if felt like quite the overkill for my purposes.

Comparing to `oguri`, `fswww` uses less cpu power to animate once it has cached
all the frames in the animation. It should also be **significantly** more
memory efficient (make sure to see the 
[Caveats/Limitations](#Caveats/Limitations) though).

The second is that, to my knowledge, there is no wallpaper daemon for wayland
that allows you to change the wallpaper at runtime. That is, is order to, for
example, cycle through the images of a directory, you'd have to kill the daemon
and restart it. Not only does it make simple shell scripts a pain to write, it
makes switch from one image to the next to happen very abruptly.

#### Usage

Start by initializing the daemon:
```
fswww init
```
Then, simply pass the image you want to display:
```
fswww img <path/to/img>

# You can also specify outputs:
fswww img -o <outputs> <path/to/img>

# And control how smoothly the transition will happen
# Smaller values = more smooth. Default = 20
fswww img --transition-step <number from 1 to 255>
```
If you would like to know the valid values for *\<outputs\>* then you can query
the daemon. This will also tell you what the current image being displayed is,
as well as the dimensions detected for the outputs. If you need more detailed
information, I would recommend using [wlr-randr](https://sr.ht/~emersion/wlr-randr/).
```
fswww query
```
Finally, to stop the daemon, kill it:
```
fswww kill
```
For a more complete description, run *fswww --help* or *fswww \<subcommand\>
--help*.

### Caveats/Limitations

I had a glorious name when I started this project, but alas, I couldn't quite
get there, here are some issues with it:

 - Despite trying my best to make this as resource efficient as possible,
 **memory can still be an issue**. In particular, from my testing, memory usage
 is annoying in the following cases:

   * Memory goes up when resizing images. I am still not entirely sure why.
   * Whenever you switch to an image that *hasn't been displayed yet*, memory
   also goes up. Note this means that, if you are cycling through a directory,
   you will only know for sure how much memory `fswww` will use when it goes
   through all the images at least once. Why is it this way? I have no idea.

- If you try to initialize `fswww` and display an image as fast as possible using
your compositor's init script, like so:
     ```
	 fswww init && fswww img <path/to/img>
	 ```
  `fswww` might fail. Further, if it does fail, it will significantly slow down
  the initializating process of your compositor. One solution is to do this
  instead:
  ```
  	(fswww init && fswww img <path/to/img>) &
  ```
  This will send the commands to the background, so if they fail it won't be a
  problem. Interestingly, for me, in river, the first version above will fail,
  and the second one succed, every time, though it does leave a zombie process
  permanently attached to the river process.

- If the daemon exits in an unexpected way (for example, if you send SIGKILL to
force its shutdown), it will leave a `fswww.socket` file behind in 
`$XDG_RUNTIME_DIR` (or `/tmp/fswww` if it isn't set). If you want to 
reinitialize the daemon, you will have to remove that file first.
