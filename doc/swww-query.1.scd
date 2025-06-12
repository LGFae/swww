swww-query(1)

# NAME
swww-query

# SYNOPSIS
*swww query*

# OPTIONS

*-a*,*--all*
	Send this command to all active `swww-daemon` namespaces.

*-n*,*--namespace* <namespace>
	Which wayland namespace to send this command to.

	You can specify this option multiple times to send commands to multiple
	namespaces at once: `swww query -n <namespace1> -n <namespace 2>`.

	This affects the name of the `swww-daemon` socket we will use to
	communicate with. Specifically, our socket name is
	_${WAYLAND_DISPLAY}-swww-daemon.<specified namespace>.socket_.

	Some compositors can have several different wallpapers per output. This
	allows you to differentiate between them. Most users will probably not have
	to set anything in this option.

*-h*, *--help*
	Print help (see a summary with '-h')

# DESCRIPTION

Asks the daemon to print output information (names and dimensions).

You may use this to find out valid values for the <swww-img --outputs> option. If
you want more detailed information about your outputs, I would recommend trying
something like *wlr-randr*.

# OUTPUT FORMAT

Currently, *swww query* prints information in the following format:

```
OUTPUT: SIZE, scale: SCALE, currently displaying: IMAGE_OR_COLOR
```

where *SIZE* is in the format *WxH* (eg.: *1920x1080*), *SCALE* in "scale:
NUMBER", and *IMAGE_OR_COLOR* in
	- "image: IMAGENAME", if it's an image; or
	- "color: RGB", if it's a color
