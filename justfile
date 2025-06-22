cluster_name := "hello"

create-cluster:
    kwokctl create cluster --name {{ cluster_name }}
    kwokctl scale node --replicas 3 --name {{ cluster_name }}

delete-cluster:
    kwokctl delete cluster --name hello
