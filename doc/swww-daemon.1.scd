swww-daemon(1)

# NAME
swww-daemon

#DESCRIPTION 

The *swww-daemon* will run continuously, waiting for commands in
_$XDG_RUNTIME_DIR/swww.socket_(or _/tmp/swww/swww.socket_, if $XDG_RUNTIME_DIR
is not set). The daemon will take care of both creating and deleting that file
when it is initialized or killed.

*There is no reason for you to run the swww-daemon manually*. The daemon should
be started through *swww init* and killed through *swww kill*. Running the
daemon manually would only help for debugging, or developing *swww*. *swww init*
has the benefits that it checks to see if another instance is already running,
and it waits for the daemon to be ready, so something like

```
swww init && swww img bg.png
```

works, but 

```
swww-daemon &
swww img bg.png
```

*can potentially fail*, because the daemon might still not be ready by the time
the client sends the image.

Bottom line is: just use *swww init* to initialize the daemon.

# SEE ALSO
*swww-init*(1)
