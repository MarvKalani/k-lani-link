FROM nginx:1.29-alpine
RUN apk add --no-cache iptables
COPY deploy/thinkrail/network-guard.sh /opt/network-guard.sh
ENTRYPOINT ["/bin/sh", "/opt/network-guard.sh"]
