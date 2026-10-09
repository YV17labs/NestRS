# syntax=docker/dockerfile:1.7

FROM node:24-slim AS builder
WORKDIR /app
COPY package.json package-lock.json nest-cli.json tsconfig.json tsconfig.build.json ./
RUN npm ci --no-audit --no-fund
COPY src ./src
RUN npm run build && npm prune --omit=dev

FROM node:24-slim AS runtime
ENV NODE_ENV=production PORT=3120
WORKDIR /app
COPY --from=builder /app/package.json ./
COPY --from=builder /app/node_modules ./node_modules
COPY --from=builder /app/dist ./dist
EXPOSE 3120
USER node
CMD ["node", "dist/main"]
