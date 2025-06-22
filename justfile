cluster_name = "hello"

create-cluster:
    kwokctl create cluster --name {{ cluster_name }} --extra-args="kubeadm=--pod-network-cidr=10.0.0.0/24"
    kwokctl scale node --replicas 3 --name {{ cluster_name }}

