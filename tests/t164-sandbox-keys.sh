#!/usr/bin/env bash
# A test's sandbox holds none of the session's keys that reach a real
# service: a hub started in a test with the session's Slack tokens posted
# its questions to Saiful's real Slack DM, under a machine no tailnet knows.
export SLACK_BOT_TOKEN=xoxb-real SLACK_APP_TOKEN=xapp-real SLACK_USER_ID=U-real
export HUB_STORE_TOKEN=real TS_AUTHKEY=real MEM_REMOTE=https://real.example
source "$(dirname -- "$0")/lib.sh"
t_init

for key in SLACK_BOT_TOKEN SLACK_APP_TOKEN SLACK_USER_ID HUB_STORE_TOKEN TS_AUTHKEY MEM_REMOTE; do
	is "${!key-unset}" unset "the sandbox does not carry $key"
done
