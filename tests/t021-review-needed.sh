#!/usr/bin/env bash
# workflow review-needed: the icase pathspec table of spec §9 and the change
# set that feeds it (status -uall ∪ diff range). AC11's fixtures live here.
source "$(dirname -- "$0")/lib.sh"
t_init

# Every fixture starts from the same quiet Laravel-shaped repo.
fresh() {
	rm -rf "$T_TMP/app-repo"
	new_repo app-repo
	mkdir -p app/Services resources/views
	printf '<?php\n' >app/Services/CartPricing.php
	printf 'x\n' >resources/views/cart.blade.php
	git add -A
	git -c core.hooksPath=/dev/null commit -qm 'cart'
}

# triggers <path> <desc> -- an untracked file at <path> wants a review.
triggers() {
	fresh
	mkdir -p "$(dirname -- "$1")"
	printf 'x\n' >"$1"
	run workflow review-needed
	is "$RC" 0 "triggers: $2"
	like "$OUT" "$(basename -- "$1")" "triggers: $2 (names the file)"
}

triggers .env 'a staged-or-untracked root .env'
triggers Dockerfile 'a root Dockerfile'
triggers app/auth/NewGuard.php 'a new file under app/auth/'
triggers app/Jobs/ProcessPayment.php 'StudlyCase app/Jobs/'
triggers app/Http/Middleware/Authenticate.php 'StudlyCase middleware named Authenticate'
triggers storage/keys/PrivateKey.pem 'StudlyCase PrivateKey.pem'
triggers app/Policies/CartPolicy.php 'StudlyCase app/Policies/'
triggers app/Services/StripeGateway.php 'StudlyCase StripeGateway'
triggers database/migrations/2026_01_01_add_column.php 'a new migration'
triggers .github/workflows/ci.yml 'a CI workflow'
triggers routes/api.php 'the api route file'
triggers docs/openapi.yaml 'an openapi document'
triggers deploy/prod/Caddyfile 'a deploy directory'

# The root .env case again, this time actually staged -- the review-3 F-1 shape.
fresh
printf 'APP_KEY=x\n' >.env
git add -f .env
run workflow review-needed
is "$RC" 0 'a staged root .env triggers'

# A change of the same size in ordinary feature code does not.
fresh
printf '<?php\n// pricing tweaks\n' >>app/Services/CartPricing.php
printf 'more\n' >>resources/views/cart.blade.php
mkdir -p app/Services
printf '<?php\n' >app/Services/CartTotals.php
run workflow review-needed
is "$RC" 1 'a same-size feature diff does not trigger'
like "$OUT" 'no' 'the quiet answer says so'

# Manifests are on the table.
fresh
printf '{"name":"n"}\n' >package.json
run workflow review-needed
is "$RC" 0 'a new package.json triggers'

## --------------------------------------------------------------- the range

fresh
mkdir -p app/Http/Middleware
printf '<?php\n' >app/Http/Middleware/Authenticate.php
git add -A
git -c core.hooksPath=/dev/null commit -qm 'guard'
run workflow review-needed
is "$RC" 1 'a clean tree with the change already committed does not trigger'
run workflow review-needed --diff HEAD~1..HEAD
is "$RC" 0 '--diff finds the committed change'

## --------------------------------------------------------- cwd independence

fresh
mkdir -p app/auth resources/views/deep
printf 'x\n' >app/auth/Guard.php
cd resources/views/deep || exit 1
run workflow review-needed
is "$RC" 0 'a subdirectory sees the same repo-root-anchored table'
cd "$T_TMP/app-repo" || exit 1

# `*` must not cross a slash: a Cart* pattern is not a licence for a subtree.
fresh
mkdir -p app/Services/Cartx
printf 'x\n' >app/Services/Cartx/Deep.php
run workflow review-needed
is "$RC" 1 'a nested file under a Cart* directory is still ordinary feature code'
