#!/bin/bash

CLICKHOUSE_DB="tracer_api_db"

clickhouse-client --query "CREATE DATABASE ${CLICKHOUSE_DB}";
if [ "$?" -ne "0" ]; then
  echo "Database already created"
  exit 0
fi

cat /etc/clickhouse-server/schema.sql | clickhouse-client -mn -d ${CLICKHOUSE_DB}

