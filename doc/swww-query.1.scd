swww-query(1)

# NAME
swww-query

# SYNOPSIS
*swww query*

# OPTIONS

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
