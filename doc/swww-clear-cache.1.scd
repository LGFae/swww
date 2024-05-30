swww-clear-cache(1)

# NAME
swww-clear-cache

# SYNOPSIS
*swww clear-cache*

# OPTIONS

*-h*, *--help*
	Print help (see a summary with '-h')

# DESCRIPTION

Deletes the `swww` cache directory.

The cache resides at _$XDG_CACHE_HOME/swww_ or _$HOME/.cache/swww_
if $XDG_CACHE_HOME does not exist. For each monitor, there will be a file in
those locations corresponding to the current image/animation being displayed.

Furthermore,  the cache will keep preprocessed versions of `gif`s. So, if you
load a large `gif`, you would have to pay the price for its processing the first
time.

Note that `swww` will automatically delete any preprocessed animation created
with a previous version of `swww` from the cache.

# SEE ALSO
*swww-img*(1)
