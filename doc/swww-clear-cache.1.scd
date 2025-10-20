swww-clear-cache(1)

# NAME
swww-clear-cache

# SYNOPSIS
*swww clear-cache*

# OPTIONS

*-h*, *--help*
	Print help (see a summary with '-h')

# DESCRIPTION

Deletes the *swww* cache directory.

The cache resides at _$XDG_CACHE_HOME/swww_ or _$HOME/.cache/swww_
if $XDG_CACHE_HOME does not exist. In this directory, we create another
directory corresponding to *swww*s current version number. In there, we have
files that cache the last displayed image and animation frames for animated
wallpapers. There is one file per output.

**Note that *swww* will automatically delete any files and directories in 
_$XDG_CACHE_HOME/swww_ that do not correspond to the exact current *swww*
version number.** This is to ensure we only keep cache files for the latest
*swww* version.

# SEE ALSO
*swww-img*(1)
