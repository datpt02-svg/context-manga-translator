FROM nvidia/cuda:12.4.1-runtime-ubuntu22.04

RUN apt-get update && apt-get install -y --no-install-recommends \
    python3 python3-pip python3-venv curl ca-certificates \
    fonts-noto-cjk fonts-noto \
    && apt-get clean && rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Copy & install deps
COPY pyproject.toml /app/
RUN python3 -m venv /venv && \
    /venv/bin/pip install --no-cache-dir uv && \
    /venv/bin/uv pip install --system -r <(/venv/bin/uv pip compile pyproject.toml 2>/dev/null || cat pyproject.toml)

# Install ms_wrapper from GitHub
RUN /venv/bin/pip install --no-cache-dir git+https://github.com/tyxsspa/anytext2.git

COPY schemas.py server.py /app/

EXPOSE 7863

ENV ANYTEXT2_MODEL_DIR=/app/models
ENV ANYTEXT2_DEVICE=cuda
ENV ANYTEXT2_FP16=1

CMD ["/venv/bin/python", "server.py"]
