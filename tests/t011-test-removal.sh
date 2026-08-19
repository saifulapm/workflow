#!/usr/bin/env bash
# workflow verify: the staged-diff test-removal check (spec §7, AC6) and the
# two ways to clear it. Also covers review-3 F-3: a partial commit hands the
# hook a temporary index, and verify must read *that* index.
source "$(dirname -- "$0")/lib.sh"
t_init

# A repo whose suite always passes, so any non-zero exit is the removal check.
green_repo() {
	new_repo "$1"
	printf '{"name":"acme/app"}\n' >composer.json
	printf '#!/bin/sh\nexit 0\n' >artisan
	chmod +x artisan
	write_exec bin/php <<-'EOF'
		#!/bin/sh
		exit 0
	EOF
	mkdir -p tests
	cat >tests/CartTest.php <<-'EOF'
		<?php
		class CartTest extends TestCase
		{
		    public function testCartTotalsAreRounded()
		    {
		        $this->assertSame(100, (new Cart)->total());
		    }

		    public function testEmptyCartIsFree()
		    {
		        $this->assertSame(0, (new Cart)->total());
		    }
		}
	EOF
	git add -A
	git -c core.hooksPath=/dev/null commit -qm 'project files'
	mem_register
}

green_repo php
git rm -q tests/CartTest.php
run workflow verify
is "$RC" 3 'php: staging a deleted test file exits 3'
like "$OUT" 'mem save --kind ruling --type test-removal' 'php: prints the exact clearing command'
like "$OUT" 'testCartTotalsAreRounded' 'php: names the tests that went away'

# Clearing #1: any test-removal ruling inside the 8h window.
"$MEM_BIN" save --kind ruling --type test-removal \
	'Cart tests move to the pricing service - why: the class is gone - cost: none' >/dev/null
run workflow verify
is "$RC" 0 'php: an 8h test-removal ruling clears it'

green_repo named
git rm -q tests/CartTest.php
# Clearing #2: a ruling that names the tests, whatever its age.
"$MEM_BIN" save --kind ruling --type test-removal \
	'testCartTotalsAreRounded and testEmptyCartIsFree are replaced by the service suite' >/dev/null
age_mem_items
run workflow verify
is "$RC" 0 'php: an out-of-window ruling naming the removed tests clears it'

green_repo unrelated
# Clearing rulings must name *the* tests: an unrelated old one does not.
"$MEM_BIN" save --kind ruling --type test-removal 'testSomethingElse was a duplicate' >/dev/null
age_mem_items
git rm -q tests/CartTest.php
run workflow verify
is "$RC" 3 'php: an out-of-window ruling naming other tests does not clear it'

# Additions are never a removal.
green_repo added
cat >tests/NewTest.php <<-'EOF'
	<?php
	class NewTest extends TestCase
	{
	    public function testSomethingNew() { $this->assertTrue(true); }
	}
EOF
git add tests/NewTest.php
run workflow verify
is "$RC" 0 'adding a test is not a removal'

# A pure rename nets zero declarations.
green_repo renamed
git mv tests/CartTest.php tests/BasketTest.php
run workflow verify
is "$RC" 0 'renaming a test file nets zero declarations'

## ------------------------------------------------- other language patterns

new_repo rust
mkdir -p src
printf '[package]\nname = "probe"\nversion = "0.0.0"\nedition = "2021"\n' >Cargo.toml
cat >src/lib.rs <<-'EOF'
	pub fn two() -> i32 { 2 }

	#[cfg(test)]
	mod tests {
	    use super::*;

	    #[test]
	    fn two_is_two() { assert_eq!(two(), 2); }

	    #[tokio::test]
	    async fn two_is_still_two() { assert_eq!(two(), 2); }
	}
EOF
git add -A
git -c core.hooksPath=/dev/null commit -qm rust
mem_register
printf 'pub fn two() -> i32 { 2 }\n' >src/lib.rs
git add src/lib.rs
run workflow verify
is "$RC" 3 'rust: removing #[test] and #[tokio::test] exits 3'
like "$OUT" 'two_is_two' 'rust: names the removed test function'

new_repo js
cat >package.json <<-'EOF'
	{"name":"n","scripts":{"test":"exit 0"}}
EOF
mkdir -p test
cat >test/cart.test.js <<-'EOF'
	describe('cart', () => {
	  it('rounds the total', () => { expect(1).toBe(1) })
	  test('is free when empty', () => { expect(0).toBe(0) })
	})
EOF
git add -A
git -c core.hooksPath=/dev/null commit -qm js
mem_register
git rm -q test/cart.test.js
run workflow verify
is "$RC" 3 'javascript: removing it()/test() blocks exits 3'
like "$OUT" 'rounds the total' 'javascript: names the removed test'

## ------------------------------------------------------- F-3: a temp index

green_repo tempindex
# What `git commit --only <path>` builds for the hook: a temporary index with
# only part of the work staged. The repo index itself stays clean.
tmpidx="$T_TMP/partial-index"
rm -f "$tmpidx"
GIT_INDEX_FILE=$tmpidx git read-tree HEAD
GIT_INDEX_FILE=$tmpidx git rm -q --cached tests/CartTest.php
rm -f tests/CartTest.php
run env GIT_INDEX_FILE="$tmpidx" workflow verify
is "$RC" 3 'partial commit: the removal staged in the temporary index is caught'
run workflow verify
isnt "$RC" 3 'partial commit: the repo index on its own has nothing staged'
