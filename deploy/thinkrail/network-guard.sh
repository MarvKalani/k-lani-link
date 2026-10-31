#!/bin/sh
set -eu
for tool in iptables ip6tables; do
    "$tool" -w -P OUTPUT DROP
    "$tool" -w -P INPUT DROP
    "$tool" -w -P FORWARD DROP
    # Docker DNATs 127.0.0.11:53 to an ephemeral loopback port before filter/OUTPUT.
    # Match the original destination port so that loopback cannot bypass the DNS block.
    "$tool" -w -A OUTPUT -p udp -m conntrack --ctorigdstport 53 -j DROP
    "$tool" -w -A OUTPUT -p tcp -m conntrack --ctorigdstport 53 -j DROP
    "$tool" -w -A OUTPUT -o lo -j ACCEPT
    "$tool" -w -A OUTPUT -m conntrack --ctstate ESTABLISHED,RELATED -j ACCEPT
    "$tool" -w -A INPUT -i lo -j ACCEPT
    "$tool" -w -A INPUT -m conntrack --ctstate ESTABLISHED,RELATED -j ACCEPT
    "$tool" -w -A INPUT -p tcp --dport 8080 -j ACCEPT
    if [ "${LINK_STATS_PORT:-}" = 8081 ]; then
        "$tool" -w -A INPUT -p tcp --dport 8081 -j ACCEPT
    fi
done
touch /tmp/guard.ready
exec tail -f /dev/null
