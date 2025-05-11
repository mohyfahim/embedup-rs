#!/bin/bash

VERSION=2

SCRIPT_DIR=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" &>/dev/null && pwd)

SRC_DIR="$SCRIPT_DIR""/update"
DEST_DIR=/root/services

echo $SCRIPT_DIR

backup() {
    local ts
    ts=$(date +%Y%m%d%H%M%S)
    local BACKUP_DIR="/root/services_backup_${ts}"

    echo "[backup] creating backup in $BACKUP_DIR"
    mkdir -p "$BACKUP_DIR"

    # filesystem backup
    cp -a "$DEST_DIR" "$BACKUP_DIR/services"

    # database backup (custom, compressed)
    PGPASSWORD="$DB_PASSWORD" pg_dump \
        -U podbox -h localhost \
        -d podbox >"$BACKUP_DIR/db_${ts}.dump"

    echo "$ts" >"$BACKUP_DIR/backup.timestamp"
}

tests() {
    echo "[tests] checking services are running"
    systemctl is-active --quiet podbox &&
        echo "  podbox: OK" ||
        {
            echo "  podbox: FAILED"
            return 1
        }

    systemctl is-active --quiet nginx &&
        echo "  nginx: OK" ||
        {
            echo "  nginx: FAILED"
            return 1
        }

    sleep 10

    echo "[tests] checking HTTP endpoints"
    local code
    code=$(curl -s -o /dev/null -w "%{http_code}" http://localhost:8000/)
    [ "$code" = "404" ] &&
        echo "  backend health: 404" ||
        {
            echo "  backend health: $code"
            return 1
        }

    code=$(curl -s -o /dev/null -w "%{http_code}" https://podbox.plus/)
    [ "$code" = "200" ] &&
        echo "  PWA root: 200" ||
        {
            echo "  PWA root: $code"
            return 1
        }

    echo "[tests] all checks passed"
}

rollback() {
    local latest
    latest=$(ls -1dt /root/services_backup_* 2>/dev/null | head -n1)
    if [[ -z "$latest" ]]; then
        echo "[rollback] no backups found under /root/"
        exit 1
    fi

    echo "[rollback] restoring from $latest"
    systemctl stop nginx podbox

    # filesystem restore
    rm -rf "$DEST_DIR"
    cp -a "$latest/services" "$DEST_DIR"

    # database restore
    local dumpfile
    dumpfile=$(ls "$latest"/db_*.dump | head -n1)
    if [[ -z "$dumpfile" ]]; then
        echo "[rollback] no DB dump found in $latest"
        exit 1
    fi
    echo "[rollback] restoring DB from $dumpfile"
    
    PGPASSWORD=$DB_PASSWORD psql -d "podbox" -U podbox -h localhost -f "$dumpfile"

    systemctl start podbox nginx
    echo "[rollback] done"
}

update_backend() {
    BACKEND_SRC_DIR="$SRC_DIR/back"
    BACKEND_OUT_DIR="$DEST_DIR/back"

    # TODO: handle env updates
    systemctl stop podbox

    rm -rf $BACKEND_OUT_DIR
    mv $BACKEND_SRC_DIR $BACKEND_OUT_DIR
    chown -R root:podbox $BACKEND_OUT_DIR
    find $BACKEND_OUT_DIR -type d -exec chmod 750 {} +
    find $BACKEND_OUT_DIR -type f -exec chmod 640 {} +
    systemctl restart podbox

}

update_pwa() {
    PWA_SRC_DIR="$SRC_DIR/pwa"
    PWA_OUT_DIR="$DEST_DIR/pwa"
    rm -rf $PWA_OUT_DIR
    mv $PWA_SRC_DIR $PWA_OUT_DIR
    chown -R root:podbox $PWA_OUT_DIR
    find $PWA_OUT_DIR -type d -exec chmod 750 {} +
    find $PWA_OUT_DIR -type f -exec chmod 640 {} +
    systemctl restart nginx
}

update_db() {
    echo "[update_db] dropping & recreating podbox database"
    systemctl stop podbox.service
    PGPASSWORD="$DB_PASSWORD" psql -d "template1" -U podbox -h localhost -c "drop DATABASE podbox WITH (FORCE);"
    PGPASSWORD="$DB_PASSWORD" psql -d "template1" -U podbox -h localhost -c "CREATE DATABASE podbox;"
    PGPASSWORD="$DB_PASSWORD" psql -d "podbox" -U podbox -h localhost -f "$SRC_DIR/db.dump"
    systemctl start podbox.service
}

update_contents() {
    if mountpoint -q /mnt/sdcard; then
        cp -r $SRC_DIR/images /mnt/sdcard/assets/
    fi
}


update_version() {
    echo "$VERSION" > /etc/podbox_update/version.txt
}

main() {
    echo "[deploy] starting at $(date)"
    backup
    update_backend
    update_pwa
    update_db
    update_contents
    update_version
    if ! tests; then
        echo "[deploy] tests failed, rolling back"
        rollback
        exit 1
    fi

    echo "[deploy] SUCCESS"
}

# entrypoint
main