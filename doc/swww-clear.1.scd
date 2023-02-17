swww-clear(1)

# NAME
swww-clear

# SYNOPSIS
*swww clear* [OPTIONS] <COLOR>

# OPTIONS

*-o*, *--outputs*
	Comma separated list of outputs to display the image at. Use *swww query* to
	know which outputs are currently being used.

	If it isn't set, the image is displayed on all outputs.

*-h*, *--help*
	Print help (see a summary with '-h')

# COLOR

The color to fill the screen with. It must be given in *RRGGBB*, hex format. Note
there is no prepended '#'. Defaults to *000000*.

# DESCRIPTION
Fills the specified outputs with the given color.

Currently, we *do not* cache this, so if you want a color to be set at
initialization, you must set it every time:

```
swww init && swww clear 1a804a
```

# SEE ALSO
*swww-query*(1)
