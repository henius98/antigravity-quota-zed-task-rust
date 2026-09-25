#!/usr/bin/env bash
set -euo pipefail

# -----------------------------------------------------------------------------
# Patch script for Google Antigravity ACP Server in Zed
# Enables Claude (claude-sonnet-4-6, claude-opus-4-6-thinking) and GPT models.
# -----------------------------------------------------------------------------

find_par_file() {
  local search_dirs=(
    "${HOME}/.var/app/dev.zed.Zed/data/zed/external_agents/registry/antigravity-acp"
    "${HOME}/.local/share/zed/external_agents/registry/antigravity-acp"
  )

  for dir in "${search_dirs[@]}"; do
    if [[ -d "$dir" ]]; then
      local found
      found=$(find "$dir" -name "agy_acp_server.par" -type f 2>/dev/null | sort -V | tail -n 1)
      if [[ -n "$found" ]]; then
        echo "$found"
        return 0
      fi
    fi
  done
  return 1
}

# Determine target file
TARGET_PAR="${1:-}"

if [[ "$TARGET_PAR" == "--restore" || "$TARGET_PAR" == "-r" ]]; then
  TARGET_PAR="${2:-}"
  RESTORE_MODE=true
else
  RESTORE_MODE=false
fi

if [[ -z "$TARGET_PAR" ]]; then
  echo "🔍 Searching for agy_acp_server.par..."
  if ! TARGET_PAR=$(find_par_file); then
    echo "❌ Error: Could not automatically find agy_acp_server.par." >&2
    echo "Usage: $0 [--restore] [/path/to/agy_acp_server.par]" >&2
    exit 1
  fi
fi

if [[ ! -f "$TARGET_PAR" ]]; then
  echo "❌ Error: File not found at $TARGET_PAR" >&2
  exit 1
fi

BACKUP_PAR="${TARGET_PAR}.bak"

# Restore mode
if [[ "$RESTORE_MODE" == "true" ]]; then
  if [[ ! -f "$BACKUP_PAR" ]]; then
    echo "❌ Error: Backup file not found at $BACKUP_PAR" >&2
    exit 1
  fi
  echo "🔄 Terminating running instances..."
  pkill -9 -f "agy_acp_server.par" 2>/dev/null || true
  pkill -9 -f "localharness_external" 2>/dev/null || true
  sleep 0.5
  echo "🔄 Restoring original file from $BACKUP_PAR..."
  cp -f "$BACKUP_PAR" "${TARGET_PAR}.tmp"
  chmod --reference="$TARGET_PAR" "${TARGET_PAR}.tmp" 2>/dev/null || chmod +x "${TARGET_PAR}.tmp"
  mv -f "${TARGET_PAR}.tmp" "$TARGET_PAR"
  echo "✅ Restored successfully. Restart Zed to reload original models."
  exit 0
fi

echo "🎯 Target file: $TARGET_PAR"

# Stop active instances before patching
echo "🔄 Stopping running ACP server instances to release executable lock..."
pkill -9 -f "agy_acp_server.par" 2>/dev/null || true
pkill -9 -f "localharness_external" 2>/dev/null || true
sleep 0.5

# Create backup if not already present
if [[ ! -f "$BACKUP_PAR" ]]; then
  echo "💾 Creating backup at $BACKUP_PAR..."
  cp -p "$TARGET_PAR" "$BACKUP_PAR"
else
  echo "ℹ️  Backup already exists at $BACKUP_PAR (retaining original backup)"
fi

echo "🛠️  Patching model selection and Claude tool schemas..."

python3 - "$TARGET_PAR" <<'PY'
import os
import shutil
import struct
import sys
import zipfile
import zlib

par_path = os.path.expanduser(sys.argv[1])
tmp_path = par_path + ".tmp"

model_selection_path = (
    "google3/cloud/developer_experience/antigravity_extensions/acp_server/"
    "model_selection.py"
)
ccpa_proxy_path = (
    "google3/cloud/developer_experience/antigravity_extensions/acp_server/"
    "ccpa_connection/proxy_server.py"
)

# Read from original
with open(par_path, 'rb') as f:
    data = bytearray(f.read())

def redirect_bytecode(archive_path):
    old_path = archive_path.encode()
    new_path = archive_path.removesuffix(".pyc").encode() + b".bak"
    old_count = data.count(old_path)
    new_count = data.count(new_path)
    if old_count == 2:
        data[:] = data.replace(old_path, new_path)
        print(f"  ✓ Disabled bytecode cache for {archive_path.rsplit('/', 1)[-1]}")
    elif old_count == 0 and new_count == 2:
        print(f"  ℹ️  Bytecode cache already disabled for {archive_path.rsplit('/', 1)[-1]}")
    else:
        print(
            f"  ❌ Error: Unexpected archive entries for {archive_path}",
            file=sys.stderr,
        )
        sys.exit(1)


# Rename precompiled files so zipimport executes the patched sources below.
redirect_bytecode(
    "google3/cloud/developer_experience/antigravity_extensions/acp_server/"
    "__pycache__/model_selection.cpython-314.pyc"
)
redirect_bytecode(
    "google3/cloud/developer_experience/antigravity_extensions/acp_server/"
    "ccpa_connection/__pycache__/proxy_server.cpython-314.pyc"
)

with zipfile.ZipFile(par_path, "r") as zf:
    model_selection = zf.read(model_selection_path).decode("utf-8")
    ccpa_proxy = zf.read(ccpa_proxy_path).decode("utf-8")

# Expose every model returned by CCPA instead of only Gemini models.
old_filter = 'if not ccpa_id.startswith("gemini"):'
new_filter = 'if not ccpa_id.startswith(""):      '
if old_filter in model_selection:
    model_selection = model_selection.replace(old_filter, new_filter, 1)
    print("  ✓ Patched filter: allowing all backend models (Claude, GPT, etc.)")
elif new_filter in model_selection:
    print("  ℹ️  Model filter is already patched")
else:
    print("  ❌ Error: Model filter pattern not found", file=sys.stderr)
    sys.exit(1)

# CCPA's Claude adapter requires functionDeclarations[].parameters as an API
# Schema, not raw JSON Schema. The bundled GenAI converter removes unsupported
# fields such as $schema. Reclaim space from a nearby comment to keep ZIP offsets.
request_comment = '''\
      # Wrap the incoming standard Gemini request body into CCPA's
      # GenerateContentRequest.
'''
short_request_comment = '''\
      # Parse CCPA request.
'''
schema_anchor = '''\
    # The proxy is authoritative for the User-Agent: it injects the value the
    # server configured (built once per connection from the client surface),
    # rather than trusting whatever the SDK forwarded on the incoming request.
    # This guarantees correct backend client-type attribution regardless of how
    # the SDK rewrites the outgoing header.
'''
schema_patch = '''\
    # Keep server User-Agent.
    if model[:6] != "gemini":
      import google.genai.types as g
      for t in incoming_json.get("tools",[]):
        for d in t.get("functionDeclarations",[]):
          s=d.pop("parametersJsonSchema",d.get("parameters",{"type":"object"}))
          d["parameters"]=g.Schema.from_json_schema(json_schema=g.JSONSchema.model_validate(s,extra="ignore")).model_dump(by_alias=True,exclude_none=True)
'''
current_patch = '''\
    # Keep server User-Agent; fix CCPA tool.
    if model[:6] != "gemini":
      for t in incoming_json.get("tools",[]):
        for d in t.get("functionDeclarations",[]):
          s=d.pop("parametersJsonSchema",d.get("parameters",{}))
          d["parameters"]=json.loads(re.sub(
              r'(?<="type": ")[a-z]+',lambda m:m[0].upper(),json.dumps(s)))
'''
previous_patch = '''\
    # Set a non-Gemini adapter's required JSON Schema tool field.
    if not model.startswith("gemini"):
      for t in incoming_json.get("tools", []):
        for d in t.get("functionDeclarations", []):
          d.setdefault("parametersJsonSchema", d.pop(
              "parameters", {"type": "object", "properties": {}, "required": []}
          ))
    #
'''
if schema_patch in ccpa_proxy:
    print("  ℹ️  Claude/GPT tool schemas are already patched")
else:
    old_patch = next(
        (patch for patch in (current_patch, previous_patch, schema_anchor)
         if patch in ccpa_proxy),
        None,
    )
    if old_patch is None or request_comment not in ccpa_proxy:
        print("  ❌ Error: CCPA proxy patch anchor not found", file=sys.stderr)
        sys.exit(1)
    if (len(schema_patch) - len(old_patch) !=
            len(request_comment) - len(short_request_comment)):
        print("  ❌ Error: Tool schema patch changed archive size", file=sys.stderr)
        sys.exit(1)
    ccpa_proxy = ccpa_proxy.replace(old_patch, schema_patch, 1)
    ccpa_proxy = ccpa_proxy.replace(request_comment, short_request_comment, 1)
    print("  ✓ Patched Claude/GPT tool schemas")


def replace_stored_member(archive_path, content):
    """Replaces a stored ZIP member without changing the executable size."""
    encoded_path = archive_path.encode()
    encoded_content = content.encode()
    eocd_pos = data.rfind(b"PK\x05\x06")
    if eocd_pos == -1:
        print("  ❌ Error: ZIP end record not found", file=sys.stderr)
        sys.exit(1)
    cd_size = struct.unpack_from("<I", data, eocd_pos + 12)[0]
    cd_offset = struct.unpack_from("<I", data, eocd_pos + 16)[0]
    zip_start = eocd_pos - cd_size - cd_offset
    cd_pos = zip_start + cd_offset
    cd_end = cd_pos + cd_size
    entry_pos = None
    while cd_pos < cd_end:
        if data[cd_pos:cd_pos + 4] != b"PK\x01\x02":
            break
        fn_len, extra_len, comment_len = struct.unpack_from(
            "<HHH", data, cd_pos + 28
        )
        filename = bytes(data[cd_pos + 46:cd_pos + 46 + fn_len])
        if filename == encoded_path:
            entry_pos = cd_pos
        cd_pos += 46 + fn_len + extra_len + comment_len
    if entry_pos is None:
        print(f"  ❌ Error: ZIP member not found: {archive_path}", file=sys.stderr)
        sys.exit(1)

    compression = struct.unpack_from("<H", data, entry_pos + 10)[0]
    old_size = struct.unpack_from("<I", data, entry_pos + 24)[0]
    local_offset = zip_start + struct.unpack_from(
        "<I", data, entry_pos + 42
    )[0]
    fn_len, extra_len = struct.unpack_from("<HH", data, local_offset + 26)
    content_offset = local_offset + 30 + fn_len + extra_len
    if compression != zipfile.ZIP_STORED or len(encoded_content) != old_size:
        print(
            f"  ❌ Error: ZIP member size changed: {archive_path}",
            file=sys.stderr,
        )
        sys.exit(1)

    data[content_offset:content_offset + old_size] = encoded_content
    crc = zlib.crc32(encoded_content)
    struct.pack_into("<I", data, local_offset + 14, crc)
    struct.pack_into("<I", data, entry_pos + 16, crc)


replace_stored_member(model_selection_path, model_selection)
replace_stored_member(ccpa_proxy_path, ccpa_proxy)

# Write to temp file first to avoid ETXTBSY
with open(tmp_path, 'wb') as f:
    f.write(data)

# Preserve permissions and executable bits
shutil.copymode(par_path, tmp_path)

print("🔍 Validating patched archive...")
with zipfile.ZipFile(tmp_path, 'r') as zf:
    bad_file = zf.testzip()
    if bad_file:
        print(f"  ❌ Zip validation failed on {bad_file}", file=sys.stderr)
        os.remove(tmp_path)
        sys.exit(1)
    print("  ✓ Archive integrity test passed!")

# Atomic replacement (allowed even if a process holds the old inode)
os.replace(tmp_path, par_path)
print("  ✓ Atomically installed patched binary!")
PY

# Kill any processes that might have started during the run
pkill -9 -f "agy_acp_server.par" 2>/dev/null || true
pkill -9 -f "localharness_external" 2>/dev/null || true

echo ""
echo "🎉 Done! The Antigravity ACP server has been patched."
echo "👉 In Zed, open the Agent Panel model dropdown (or restart Zed)."
echo "   'Claude Sonnet 4.6 (Thinking)' and 'Claude Opus 4.6 (Thinking)' are now selectable!"
