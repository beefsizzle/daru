#!/bin/sh
# memory of a process by name: dev/mem.sh amane-shell
# PSS is the honest number (shared pages split between the processes using them). GPU numbers come from the DRM fdinfo:
# Intel reports drm-total-system0, AMD drm-total-gtt and drm-total-vram (older kernels drm-memory-gtt/vram). a client
# can be open under several fds, so each client id is counted once. on an Intel GPU this memory is system RAM that
# no process's RSS or PSS includes
name="${1:-amane-shell}"

# the kernel keeps only the first 15 characters of a process's name (Umbriel's .umbriel-wrapped is .umbriel-wrappe)
for pid in $(pgrep -x "$(printf '%s' "$name" | cut -c1-15)"); do
    pss=$(awk '/^Pss:/{s+=$2} END{print s}' /proc/$pid/smaps_rollup 2>/dev/null)
    rss=$(awk '/^VmRSS:/{print $2}' /proc/$pid/status)
    anon=$(awk '/^Anonymous:/{s+=$2} END{print s}' /proc/$pid/smaps_rollup 2>/dev/null)
    threads=$(awk '/^Threads:/{print $2}' /proc/$pid/status)
    gpu=$(awk '
        FNR == 1 { id = "" }
        /^drm-client-id:/ { id = $2 }
        /^drm-(total|memory)-/ && id != "" {
            region = $1
            sub(/^drm-(total|memory)-/, "", region)
            sub(/:$/, "", region)
            kib = $2
            if ($3 == "MiB") kib *= 1024
            else if ($3 == "GiB") kib *= 1048576
            else if ($3 == "") kib /= 1024
            seen[id, region] = kib
            regions[region] = 1
        }
        END {
            for (key in seen) { split(key, part, SUBSEP); total[part[2]] += seen[key] }
            for (region in regions) if (total[region] >= 1024) printf "%s=%d MiB ", region, total[region] / 1024
        }' /proc/$pid/fdinfo/* 2>/dev/null)
    echo "$name pid=$pid  PSS=$((pss/1024)) MiB  RSS=$((rss/1024)) MiB  anon=$((anon/1024)) MiB  threads=$threads  gpu: ${gpu:-none}"
done
