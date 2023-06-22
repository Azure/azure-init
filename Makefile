build-all:
	@cargo build --all

RANDOM_NUMBER := $$((1 + RANDOM % 1000))
TEST_USER := "test_user_$(RANDOM_NUMBER)"

e2e-test: build-all
	@./target/debug/functional_tests $(TEST_USER) 
	@userdel $(TEST_USER)
	@rm -rf /home/$(TEST_USER)
	@echo "User was successfully deleted"