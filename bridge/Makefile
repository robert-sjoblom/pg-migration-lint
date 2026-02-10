IMAGE_NAME := pg-migration-lint-bridge
CONTAINER_NAME := bridge-extract
JAR_NAME := liquibase-bridge.jar
OUTPUT_DIR := ../tools

.PHONY: all build extract clean image-only

all: extract

# Build the Docker image containing the fat JAR
build:
	docker build -t $(IMAGE_NAME) .

# Build the image and extract the JAR to tools/
extract: build
	mkdir -p $(OUTPUT_DIR)
	docker create --name $(CONTAINER_NAME) $(IMAGE_NAME) 2>/dev/null || true
	docker cp $(CONTAINER_NAME):/app/$(JAR_NAME) $(OUTPUT_DIR)/$(JAR_NAME)
	docker rm $(CONTAINER_NAME) 2>/dev/null || true
	@echo ""
	@echo "JAR extracted to: $(OUTPUT_DIR)/$(JAR_NAME)"

# Build only the Docker image (no JAR extraction)
image-only: build
	@echo "Docker image built: $(IMAGE_NAME)"

# Remove extracted JAR and Docker artifacts
clean:
	rm -f $(OUTPUT_DIR)/$(JAR_NAME)
	-docker rmi $(IMAGE_NAME) 2>/dev/null
	-docker rm $(CONTAINER_NAME) 2>/dev/null
