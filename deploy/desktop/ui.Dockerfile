FROM nginx:1.29-alpine
COPY deploy/desktop/nginx.conf /etc/nginx/nginx.conf
