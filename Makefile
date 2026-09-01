# The dev loop keeps its state in the repo, as it always has: WYVEN_DATA_DIR
# pins `saves/`, `profile.toml`, `ops.toml` and `authkeys.toml` to the working
# directory. Without it the game writes to the OS application-data directory,
# which is right for a player and surprising for a checkout.
run:
	WYVEN_DATA_DIR=. WYVEN_BOOT_INGAME=1 WYVEN_WORLD=test cargo run

build:
	WYVEN_AUTH_URL=https://auth.wyvencraft.com cargo build --release

run-release:
	WYVEN_DATA_DIR=. ./target/release/wyvencraft

# What the launcher does: state in the application-data directory, assets from
# the working directory. Use this to check the shipped layout by hand.
run-release-packaged:
	./target/release/wyvencraft

# A release tarball ships the compiled form of ~460 MIT/BSD/Apache dependencies,
# and those licenses require their texts to travel with a binary distribution.
# The release workflow copies THIRD-PARTY.txt into every archive; regenerate it
# whenever Cargo.lock moves.
licenses:
	python3 scripts/third-party-notices.py > THIRD-PARTY.txt
	@echo "wrote THIRD-PARTY.txt ($$(grep -m1 ' components\.$$' THIRD-PARTY.txt))"

licenses-check:
	@python3 scripts/third-party-notices.py --check

.PHONY: run build run-release run-release-packaged licenses licenses-check
