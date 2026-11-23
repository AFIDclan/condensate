#!/usr/bin/env bash
# Install Whisper (whisper.cpp) for transcribing feedback recordings, and point the Condensate
# server at it. Run on the server (oort) as the user the service runs as:
#
#   bash deploy/install-whisper.sh [model]
#
# model defaults to large-v3-turbo-q5_0 (good accuracy, about 550 MB). On a CPU without a GPU,
# small.en or base.en are much faster but less accurate. Models:
# https://huggingface.co/ggerganov/whisper.cpp
#
# Builds whisper.cpp in ~/.local/whisper (needs git, cmake and a C++ compiler), downloads the
# model, adds CONDENSATE_WHISPER and CONDENSATE_WHISPER_MODEL to
# ~/condensate-server/condensate.env, and restarts the service. Recordings made before this are
# transcribed when the server starts.
set -euo pipefail

MODEL="${1:-large-v3-turbo-q5_0}"
DIR="$HOME/.local/whisper"
ENV_FILE="$HOME/condensate-server/condensate.env"

mkdir -p "$DIR"
if [ ! -d "$DIR/src" ]; then
  git clone --quiet https://github.com/ggml-org/whisper.cpp "$DIR/src"
fi
cd "$DIR/src"
git fetch --quiet --tags
TAG=$(git describe --tags "$(git rev-list --tags --max-count=1)")
git checkout --quiet "$TAG"
echo "Building whisper.cpp $TAG..."
cmake -B build -DCMAKE_BUILD_TYPE=Release -DWHISPER_BUILD_TESTS=OFF -DGGML_NATIVE=ON >/dev/null
cmake --build build -j"$(nproc)" --config Release --target whisper-cli >/dev/null
CLI="$DIR/src/build/bin/whisper-cli"

if [ ! -f "$DIR/ggml-$MODEL.bin" ]; then
  echo "Downloading the $MODEL model..."
  sh ./models/download-ggml-model.sh "$MODEL" "$DIR" >/dev/null 2>&1
fi
MODEL_FILE="$DIR/ggml-$MODEL.bin"

echo "Checking it works..."
"$CLI" -m "$MODEL_FILE" -f samples/jfk.wav -np -nt -t "$(nproc)" 2>/dev/null | head -3

touch "$ENV_FILE"
sed -i '/^CONDENSATE_WHISPER=/d; /^CONDENSATE_WHISPER_MODEL=/d' "$ENV_FILE"
printf 'CONDENSATE_WHISPER=%s\nCONDENSATE_WHISPER_MODEL=%s\n' "$CLI" "$MODEL_FILE" >>"$ENV_FILE"
echo "Wrote $ENV_FILE"

if systemctl is-active --quiet condensate; then
  sudo systemctl restart condensate
  echo "Restarted the condensate service."
fi
echo "Done. Feedback recordings will now be transcribed."
