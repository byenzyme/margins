FROM node:22-bookworm

RUN apt-get update && apt-get install -y --no-install-recommends \
    libatk1.0-0 libatk-bridge2.0-0 libcups2 libcairo2 libdbus-1-3 \
    libdrm2 libgbm1 libglib2.0-0 libgtk-3-0 libnspr4 libnss3 \
    libpango-1.0-0 libx11-6 libx11-xcb1 libxcb1 libxcomposite1 \
    libxdamage1 libxext6 libxfixes3 libxrandr2 libxshmfence1 \
    libasound2 fonts-liberation ca-certificates \
 && rm -rf /var/lib/apt/lists/*
