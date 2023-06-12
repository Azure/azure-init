#!/bin/bash

random_number=$((1 + RANDOM % 1000))

test_user="test_user_${random_number}"

./target/debug/functional_tests "${test_user}"

if id "$test_user" > /dev/null 2>&1; then
	echo "User was successfully created"
else
	echo "User creation failed"
	exit 1
fi

userdel "$test_user"
rm -rf /home/"$test_user"

echo "User was successfully deleted"
