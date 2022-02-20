Bug fixes are obviously welcome. Also if I did anything terribly wrong in terms
of code quality you may point it out and try to fix it.

The primary reason I made this file is to explain that **NEW FEATURES WILL NOT
BE ADDED, UNLESS THEY ARE EGREGIOUSLY SIMPLE**. I made swww with the specific
usecase of making shell scripts in mind. So, for example, stuff like timed
wallpapers, or a setup that loads a different image at different times of the
day, and so on, should all be done by combining swww with other programs.

That said, I am considering adding an option to set the default transition-step
and transition-fps values with an environment variable. I will probably end up
doing that eventually, since it makes calling the `swww img` a lot less
painful if you don't like the defaults.

So, if you want a new feature within swww itself, besides the envvar thing I 
meantioned above, I'd recommend forking it.
