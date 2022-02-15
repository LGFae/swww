#!/bin/sh
#
# This is script is necessary because, by default, the output of clap generate
# will give us an autocomplete file that won't suggest any files when you write
# <fswww img>
# and then press 'TAB'.
#
# Bash seems to be fine, and I haven't tested neither fish nor elvish
#
printf  "':path:->files' \\
&& ret=0
case \$state in
	files) # Only complete the files we support with the image crate
		_files -g \"*.png|*.jpg|*.jpeg|*.gif|*.bmp|*.tif|*.tiff|*.ico|*.webp|*.avif|*.pnm|*.pbm|*.pgm|*.ppm|*.dds|*.tga|*.exr|*.ff|*.farbfeld\"
		;;
esac" | sed \
	-e '/:path .*/r /dev/stdin' \
	-e 's/esac&& ret=0/esac/1' \
	-e '/:path .*/d' completions/_fswww \
	> completions/tmp || exit 1

sed 's/esac&& ret=0/esac/1' completions/tmp > completions/_fswww
rm completions/tmp
