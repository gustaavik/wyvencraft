run:
	WYVEN_BOOT_INGAME=1 WYVEN_WORLD=test cargo run

build:
	WYVEN_AUTH_URL=http://llzdmervhd2eyewlrapa8jhi.100.94.237.98.sslip.io cargo build --release

run-release:
	./target/release/wyvencraft