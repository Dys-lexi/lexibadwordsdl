#!/bin/sh
set -eu

if [ "${MAX_DOWNLOAD_SPEED:-0}" != 0 ]; then
    interface=$(
        { ip -o route show default; ip -o -6 route show default; } |
            awk '{ for (i = 1; i < NF; i++) if ($i == "dev") { print $(i + 1); exit } }'
    )
    if [ -z "$interface" ]; then
        echo "slowdown: could not find an interface with a default route" >&2
        exit 1
    fi

    queue_device=ifb-haste
    if ! ip link show dev "$queue_device" >/dev/null 2>&1 &&
       ! ip link add dev "$queue_device" type ifb; then
        echo "slowdown: could not create $queue_device. Check NET_ADMIN and the host's ifb driver." >&2
        exit 1
    fi

    # Allow four milliseconds of traffic or a full coalesced packet to burst.
    burst_bytes=$(awk -v speed="$MAX_DOWNLOAD_SPEED" 'BEGIN {
        bytes = speed * 500
        printf "%.0f\n", (bytes < 65536 ? 65536 : bytes)
    }')
    if ! tc qdisc replace dev "$queue_device" root tbf \
           rate "${MAX_DOWNLOAD_SPEED}mbit" burst "$burst_bytes" latency 100ms ||
       ! ip link set dev "$queue_device" up ||
       ! tc qdisc replace dev "$interface" clsact; then
        echo "slowdown: could not set up the ingress queue for $interface" >&2
        exit 1
    fi

    # A matchall filter must be deleted before it can be installed again.
    tc filter del dev "$interface" ingress protocol all pref 1 handle 1 matchall 2>/dev/null || :
    if ! tc filter add dev "$interface" ingress protocol all pref 1 handle 1 matchall \
           action mirred egress redirect dev "$queue_device"; then
        echo "slowdown: could not redirect ingress on $interface to $queue_device" >&2
        exit 1
    fi
fi

exec haste-server "$@"
