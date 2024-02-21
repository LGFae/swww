BINARY = swww
RELEASE_DIR = target/release
LOCAL_BIN = /usr/bin

.PHONY: all install clean

all:
	cargo build --release

install: all
	su -c "cp $(RELEASE_DIR)/{$(BINARY), $(BINARY)-daemon} $(LOCAL_BIN)"

clean:
	cargo clean
	su -c "rm -f $(LOCAL_BIN)/{$(BINARY),$(BINARY)-daemon}"
