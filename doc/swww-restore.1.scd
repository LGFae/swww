swww-restore(1)

# NAME
swww-restore

# SYNOPSIS
*swww restore*

# OPTIONS

*-o*, *--outputs*
	Comma separated list of outputs to restore. Use *swww query* to know which
	outputs are currently being used.

	If it isn't set, all outputs will be restored.

*-h*, *--help*
	Print help (see a summary with '-h')

# DESCRIPTION

Restores the last displayed image on the specified outputs.

This can be used to split initialization (with `swww init --no-daemon`) and
cache loading into different steps, in order to avoid race condition traps.

You can also use this command to restore the last displayed image when
reconnecting a monitor.

# SEE ALSO
*swww-clear-cache*(1)
