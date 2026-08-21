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

## ------------------------------------------------------ the Shopify surface

# Measured against ~/Sites/github/shopify_apps, where the table used to see
# almost none of the app surface (friction #HK2PNTR4).
triggers shopify.app.toml 'the app config, which carries scopes and webhook urls'
triggers apps/splitroute/shopify.app.toml 'the same file inside a monorepo'
triggers apps/splitroute/shopify.app.staging.toml 'a per-environment app config'
triggers app/routes/webhooks/orders.paid.tsx 'a file inside a webhooks directory'
triggers app/routes/Webhooks/OrdersPaid.tsx 'and the StudlyCase spelling of it'
triggers tests/webhooks.test.ts 'a webhook test beside the code'
triggers app/lib/billing.server.ts 'billing as a file rather than a directory'
triggers app/lib/usage-billing.server.ts 'a billing file with a prefix'
triggers app/billing/Charge.php 'billing as a directory, which still counts'
triggers app/lib/shopify/session-storage.server.ts 'offline session storage'
triggers db/schema/sessions.ts 'the table those sessions live in'

# A Shopify app has ordinary code too, and it must stay quiet.
fresh
mkdir -p app/components
printf 'x\n' >app/components/OrderTable.tsx
run workflow review-needed
is "$RC" 1 'an ordinary component in a Shopify app does not trigger'

## ------------------------------------------------ per-project review paths

# packages/shopify-core/** is that repo's blast radius, not everyone's, so it
# is declared per project and merged with the global table at check time.
fresh
mem_register
mkdir -p packages/shopify-core/src
printf 'x\n' >packages/shopify-core/src/client.ts
run workflow review-needed
is "$RC" 1 'a path no table claims is quiet to begin with'

"$MEM_BIN" project set review-paths 'packages/shopify-core/** scripts/mutate.py' >/dev/null
run workflow review-needed
is "$RC" 0 'once the project declares it, the same change wants a review'
like "$OUT" 'packages/shopify-core/\*\*' 'and the row that caught it is the project row'
like "$OUT" 'client.ts' 'which names the file'

# Declaring project rows adds to the global table, never replaces it.
fresh
mem_register
"$MEM_BIN" project set review-paths 'packages/shopify-core/**' >/dev/null
printf 'APP_KEY=x\n' >.env
run workflow review-needed
is "$RC" 0 'the global rows still fire alongside the project rows'

# A glob with a space in it survives the split, same grammar as Files:.
fresh
mem_register
"$MEM_BIN" project set review-paths '"config/deploy notes/**"' >/dev/null
mkdir -p 'config/deploy notes'
printf 'x\n' >'config/deploy notes/plan.md'
run workflow review-needed
is "$RC" 0 'a quoted glob keeps its space'
