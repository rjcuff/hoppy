#!/usr/bin/env bash
# Stand-in for hoppy used only to record assets/demo.gif.
# It prints hoppy's real output format with made-up addresses and processes,
# so the demo never shows anyone's actual network.

g=$'\e[32m' y=$'\e[33m' c=$'\e[36m' b=$'\e[1m' d=$'\e[2m' r=$'\e[0m'

overview() {
    cat <<EOF

  ${g}●${r}  ${b}en0${r}    Wi-Fi     192.168.1.42/24  ${d}gw${r} 192.168.1.1  ${g}← internet${r}
  ${g}●${r}  ${b}en7${r}    Ethernet  169.254.18.7/16  ${d}no gateway${r}
  ${g}●${r}  ${b}utun3${r}  VPN       10.8.0.2/24      ${d}no gateway${r}

  ${d}DNS${r}  192.168.1.1, 1.1.1.1

  ${d}Listening${r}
    ${b}:22${r}    ${c}sshd${r}      whole network      ${d}ssh${r}
    ${b}:3000${r}  ${c}node${r}      whole network      ${d}dev server${r}
    ${b}:5432${r}  ${c}postgres${r}  this machine only  ${d}postgres${r}
    ${b}:8080${r}  ${c}python3${r}   whole network      ${d}dev server${r}

  ${y}!${r} en7 has a self-assigned address (169.254.18.7): nothing gave it an IP, check cable or DHCP (normal on a Dante/AV network with no DHCP).

${d}  More: hoppy ports · hoppy port <n> · hoppy doctor [target]${r}
EOF
}

port_kill() {
    cat <<EOF

  ${d}PORT${r}  ${d}PROTO${r}  ${d}PROCESS${r}  ${d}PID${r}    ${d}REACHABLE FROM${r}
  ${b}3000${r}  tcp    ${c}node${r}     48213  ${y}whole network${r}   ${d}dev server${r}

EOF
    printf '  Kill node (PID 48213)? [y/N] '
    read -r _
    sleep 0.4
    echo "  ${g}✓${r} Stopped node (PID 48213). :3000 is free."
}

doctor() {
    echo
    echo "  ${g}✓${r} ${b}connected${r}  en0 has 192.168.1.42"
    sleep 0.3
    echo "  ${g}✓${r} ${b}router${r}     192.168.1.1 answered${d}  2 ms${r}"
    sleep 0.3
    echo "  ${g}✓${r} ${b}internet${r}   reached 1.1.1.1${d}  11 ms${r}"
    sleep 0.3
    echo "  ${g}✓${r} ${b}dns${r}        github.com is 140.82.113.4${d}  15 ms${r}"
    sleep 0.3
    echo "  ${g}✓${r} ${b}target${r}     github.com:443 is open${d}  35 ms${r}"
    echo
    echo "  ${g}✓${r} ${b}Everything works, and github.com:443 is reachable.${r}"
}

case "${1-}" in
"") overview ;;
port) port_kill ;;
doctor) doctor ;;
*) echo "demo.sh: unknown command: $1" >&2 && exit 1 ;;
esac
