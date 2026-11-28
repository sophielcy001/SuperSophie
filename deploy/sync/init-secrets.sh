#!/bin/sh
# SPDX-License-Identifier: AGPL-3.0-or-later
# 生成数据库密码与管理令牌（各 48 个字符），已有的文件保持不变。
set -eu
cd "$(dirname "$0")"
umask 077
mkdir -p secrets
chmod 700 secrets
for name in postgres_password admin_token; do
  if [ -s "secrets/$name" ]; then
    echo "保留已有的 secrets/$name"
    continue
  fi
  value=$(head -c 96 /dev/urandom | base64 | tr -dc 'A-Za-z0-9' | cut -c1-48)
  printf '%s\n' "$value" > "secrets/$name"
  # 容器内的服务用户需要读取；目录本身仅限当前用户访问。
  chmod 644 "secrets/$name"
  echo "已生成 secrets/$name"
done
if [ ! -f .env ]; then
  cp .env.example .env
  echo "已创建 .env，请填写 ZHIYE_SYNC_DOMAIN"
fi
