build-program-autara:
	./autara-deploy/scripts/build-sbf-checked.sh programs/autara-program

build-program-oracle:
	./autara-deploy/scripts/build-sbf-checked.sh programs/autara-oracle

program-test:
	cargo nextest run --no-fail-fast -j 24 -p autara-integration-tests

lib-test:
	cargo nextest run --no-fail-fast -p autara-lib

deploy: build-program-autara build-program-oracle
	cargo run --bin deploy
